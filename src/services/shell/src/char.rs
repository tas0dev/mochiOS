use swiftlib::{fs, io, ipc, process, task, vga};

// 色の編集がだるっちいったらありゃしないのでgeminiに作ってもらったエディタを使ってください。
// https://gemini.google.com/share/02481dc7584f

const FONT_WIDTH: usize = 6;
const FONT_HEIGHT: usize = 12;
const ASCII_START: usize = 32;
const ASCII_END: usize = 127;
const GLYPH_COUNT: usize = ASCII_END - ASCII_START;
const DEFAULT_FG: u32 = 0x00FF_FFFF;
const DEFAULT_BG: u32 = 0x0000_0000;
const ANSI_MAX_SEQ_LEN: usize = 32;
const ANSI_COLOR_NORMAL: [u32; 8] = [
    0x0000_0000, // black
    0x00EE_0000, // red
    0x0000_AA00, // green
    0x00AA_AA00, // yellow
    0x0000_99FF, // blue
    0x00AA_00AA, // magenta
    0x0000_AAAA, // cyan
    0x00AA_AAAA, // white
];
const ANSI_COLOR_BRIGHT: [u32; 8] = [
    0x0055_5555, // bright black (gray)
    0x00FF_5555, // bright red
    0x0055_FF55, // bright green
    0x00FF_FF55, // bright yellow
    0x0055_55FF, // bright blue
    0x00FF_55FF, // bright magenta
    0x0055_FFFF, // bright cyan
    0x00FF_FFFF, // bright white
];
const FONT_BIN_PATH: &str = "/System/fonts/ter-u12b.bin";
const FONT_BDF_PATH: &str = "/System/fonts/ter-u12b.bdf";
const ENV_FILE_PATH: &str = "/Config/env.txt";
const FONT_BIN_SIZE: usize = GLYPH_COUNT * FONT_HEIGHT;
const FONT_BDF_MAX_SIZE: usize = 512 * 1024;
const ENV_FILE_MAX_SIZE: usize = 4096;
const FONT_READ_CHUNK: usize = 512;
const FS_PATH_MAX: usize = 128;
const FS_REQ_TIMEOUT_MS: u64 = 2000;
const IPC_MSG_MAX: usize = 4128;
const PENDING_IPC_CAPACITY: usize = 32;

#[repr(C)]
#[derive(Clone, Copy)]
struct FsRequest {
    op: u64,
    arg1: u64,
    arg2: u64,
    path: [u8; FS_PATH_MAX],
}

impl FsRequest {
    const OP_OPEN: u64 = 1;
    const OP_READ: u64 = 2;
    const OP_CLOSE: u64 = 4;
    const OP_EXEC: u64 = 5;
    const OP_STAT: u64 = 6;
    const OP_READDIR: u64 = 8;
}

/// FS service の STAT/FSTAT が返す mode のファイルタイプビット
const S_IFMT: u64 = 0o170000;
const S_IFDIR: u64 = 0o040000;
const S_IFREG: u64 = 0o100000;

#[repr(C)]
#[derive(Clone, Copy)]
struct FsResponse {
    status: i64,
    len: u64,
    data: [u8; swiftlib::fs_consts::FS_DATA_MAX],
}

#[derive(Clone, Copy)]
struct PendingIpcMessage {
    used: bool,
    sender: u64,
    len: usize,
    data: [u8; IPC_MSG_MAX],
}

impl PendingIpcMessage {
    const fn new() -> Self {
        Self {
            used: false,
            sender: 0,
            len: 0,
            data: [0; IPC_MSG_MAX],
        }
    }
}

static mut PENDING_IPC_MESSAGES: [PendingIpcMessage; PENDING_IPC_CAPACITY] =
    [PendingIpcMessage::new(); PENDING_IPC_CAPACITY];

fn enqueue_pending_message(sender: u64, data: &[u8], len: usize) -> bool {
    let copy_len = core::cmp::min(len, core::cmp::min(data.len(), IPC_MSG_MAX));
    unsafe {
        for slot in &mut PENDING_IPC_MESSAGES {
            if !slot.used {
                slot.used = true;
                slot.sender = sender;
                slot.len = copy_len;
                if copy_len > 0 {
                    slot.data[..copy_len].copy_from_slice(&data[..copy_len]);
                }
                return true;
            }
        }
    }
    false
}

fn take_pending_message(buf: &mut [u8]) -> Option<(u64, usize)> {
    unsafe {
        for slot in &mut PENDING_IPC_MESSAGES {
            if slot.used {
                let copy_len = core::cmp::min(slot.len, buf.len());
                if copy_len > 0 {
                    buf[..copy_len].copy_from_slice(&slot.data[..copy_len]);
                }
                let sender = slot.sender;
                slot.used = false;
                slot.sender = 0;
                slot.len = 0;
                return Some((sender, copy_len));
            }
        }
    }
    None
}

fn read_file(path: &str, max_size: usize) -> Option<Vec<u8>> {
    if max_size == 0 {
        return None;
    }

    let fd = io::open(path, io::O_RDONLY);
    if fd < 0 {
        return None;
    }

    let mut out = Vec::new();
    let mut chunk = [0u8; FONT_READ_CHUNK];
    while out.len() < max_size {
        let read_len = core::cmp::min(chunk.len(), max_size - out.len());
        let n = io::read(fd as u64, &mut chunk[..read_len]);
        if (n as i64) < 0 {
            let _ = io::close(fd as u64);
            return None;
        }
        let n = n as usize;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&chunk[..n]);
    }

    let _ = io::close(fd as u64);
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

fn read_file_from_fs(path: &str, max_size: usize) -> Option<Vec<u8>> {
    read_file(path, max_size)
}

fn encode_exec_path_and_args(path: &str, args: &[&str]) -> Option<[u8; FS_PATH_MAX]> {
    let mut out = [0u8; FS_PATH_MAX];
    let path_bytes = path.as_bytes();
    if path_bytes.is_empty() || path_bytes.len() + 1 > FS_PATH_MAX {
        return None;
    }
    out[..path_bytes.len()].copy_from_slice(path_bytes);
    let mut pos = path_bytes.len() + 1; // path の終端 NUL

    for arg in args {
        let b = arg.as_bytes();
        if b.is_empty() {
            continue;
        }
        if pos + b.len() + 1 > FS_PATH_MAX {
            return None;
        }
        out[pos..pos + b.len()].copy_from_slice(b);
        pos += b.len();
        out[pos] = 0;
        pos += 1;
    }
    Some(out)
}

fn exec_via_fs_service(path: &str, args: &[&str]) -> Result<u64, i64> {
    process::exec_with_args(path, args).map_err(|_| -2)
}

/// OP_STAT 経由でファイルの (mode, size) を取得
fn stat_via_fs_service(path: &str) -> Option<(u64, u64)> {
    let fd = io::open(path, io::O_RDONLY);
    if fd < 0 {
        return None;
    }

    let mut dirbuf = [0u8; 4096];
    let n = fs::readdir(fd as u64, &mut dirbuf);
    if (n as i64) > 0 {
        let _ = io::close(fd as u64);
        return Some((0x4000 | 0o755, 0));
    }

    let mut size = 0u64;
    let mut buf = [0u8; 4096];
    loop {
        let n = io::read(fd as u64, &mut buf);
        if (n as i64) < 0 {
            let _ = io::close(fd as u64);
            return None;
        }
        if n == 0 {
            break;
        }
        size = size.saturating_add(n);
    }
    let _ = io::close(fd as u64);
    Some((0x8000 | 0o755, size))
}

/// OP_READDIR をページネーションしながら全エントリ名を取得
fn readdir_all_via_fs_service(path: &str) -> Option<Vec<String>> {
    let fd = io::open(path, io::O_RDONLY);
    if fd < 0 {
        return None;
    }
    let mut entries: Vec<String> = Vec::new();
    let mut buf = [0u8; 4096];
    let n = fs::readdir(fd as u64, &mut buf);
    let _ = io::close(fd as u64);
    if (n as i64) <= 0 {
        return None;
    }
    for chunk in buf[..n as usize].split(|&b| b == b'\n') {
            if chunk.is_empty() {
                continue;
            }
            if let Ok(s) = core::str::from_utf8(chunk) {
                entries.push(s.to_string());
            }
    }
    Some(entries)
}

/// CWD を基準にパスを絶対化する。 "." "/foo" "../bar" などを解決。
fn resolve_path(arg: &str) -> String {
    let abs = if arg.starts_with('/') {
        arg.to_string()
    } else {
        let mut cwd_buf = [0u8; 256];
        let cwd = fs::getcwd(&mut cwd_buf).unwrap_or("/");
        if arg.is_empty() || arg == "." {
            return cwd.to_string();
        }
        if cwd == "/" {
            format!("/{}", arg)
        } else {
            format!("{}/{}", cwd, arg)
        }
    };

    // "." と ".." を正規化
    let mut stack: Vec<&str> = Vec::new();
    for comp in abs.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    if stack.is_empty() {
        "/".to_string()
    } else {
        let mut out = String::new();
        for c in &stack {
            out.push('/');
            out.push_str(c);
        }
        out
    }
}

/// ASCII 文字ごとの 12 行ビットマップ (インデックス = codepoint - 32)
pub struct Font {
    glyphs: [[u8; FONT_HEIGHT]; GLYPH_COUNT],
}

impl Font {
    fn fallback() -> Self {
        let mut glyphs = [[0u8; FONT_HEIGHT]; GLYPH_COUNT];
        for (i, glyph) in glyphs.iter_mut().enumerate() {
            let ch = (ASCII_START + i) as u8;
            if ch == b' ' {
                continue;
            }
            glyph[0] = 0xFC;
            glyph[FONT_HEIGHT - 1] = 0xFC;
            for row in glyph.iter_mut().take(FONT_HEIGHT - 1).skip(1) {
                *row = 0x84;
            }
        }
        Font { glyphs }
    }

    fn load_from_binary() -> Option<Self> {
        match read_file_from_fs(FONT_BIN_PATH, FONT_BIN_SIZE) {
            Some(data) => {
                if data.len() < FONT_BIN_SIZE {
                    println!("[SHELL] Font binary too small: {} bytes", data.len());
                    return None;
                }
                let mut glyphs = [[0u8; FONT_HEIGHT]; GLYPH_COUNT];
                for (i, glyph) in glyphs.iter_mut().enumerate() {
                    let start = i * FONT_HEIGHT;
                    glyph.copy_from_slice(&data[start..start + FONT_HEIGHT]);
                }
                Some(Font { glyphs })
            }
            None => {
                println!("[SHELL] Font binary not found or read failed: {}", FONT_BIN_PATH);
                None
            }
        }
    }

    fn load_from_bdf() -> Option<Self> {
        match read_file_from_fs(FONT_BDF_PATH, FONT_BDF_MAX_SIZE) {
            Some(data) => {
                let mut glyphs = [[0u8; FONT_HEIGHT]; GLYPH_COUNT];
                parse_bdf(&data, &mut glyphs);
                Some(Font { glyphs })
            }
            None => {
                println!("[SHELL] BDF font not found or read failed: {}", FONT_BDF_PATH);
                None
            }
        }
    }

    /// `System/fonts/ter-u12b.bin` を優先し、失敗時はBDFを解析する
    pub fn load() -> Option<Self> {
        if let Some(font) = Self::load_from_binary() {
            return Some(font);
        }
        if let Some(font) = Self::load_from_bdf() {
            return Some(font);
        }
        println!("[SHELL] Using fallback font");
        Some(Self::fallback())
    }

    fn glyph(&self, ch: u8) -> &[u8; FONT_HEIGHT] {
        let idx = if ch >= ASCII_START as u8 && ch < ASCII_END as u8 {
            (ch as usize) - ASCII_START
        } else {
            ('?' as usize) - ASCII_START
        };
        &self.glyphs[idx]
    }
}

/// BDF データから ASCII グリフを解析して `glyphs` に書き込む
fn parse_bdf(data: &[u8], glyphs: &mut [[u8; FONT_HEIGHT]; GLYPH_COUNT]) {
    let text = core::str::from_utf8(data).unwrap_or("");
    let mut lines = text.lines();
    let mut encoding: Option<usize> = None;
    let mut in_bitmap = false;
    let mut row = 0usize;

    loop {
        let line = match lines.next() {
            Some(l) => l.trim(),
            None => break,
        };
        if line.starts_with("ENCODING ") {
            encoding = line[9..].trim().parse::<usize>().ok();
            in_bitmap = false;
            row = 0;
        } else if line == "BITMAP" {
            in_bitmap = true;
            row = 0;
        } else if line == "ENDCHAR" {
            in_bitmap = false;
            encoding = None;
            row = 0;
        } else if in_bitmap {
            if let Some(enc) = encoding {
                if enc >= ASCII_START && enc < ASCII_END {
                    let idx = enc - ASCII_START;
                    if row < FONT_HEIGHT {
                        if let Ok(byte) = u8::from_str_radix(line, 16) {
                            glyphs[idx][row] = byte;
                        }
                        row += 1;
                    }
                }
            }
        }
    }
}

/// フレームバッファへの書き込みを管理するターミナル
pub struct Terminal {
    fb_ptr: *mut u32,
    width: u32,
    height: u32,
    stride: u32,
    col: u32,
    row: u32,
    max_cols: u32,
    max_rows: u32,
    font: Font,
    pub fg: u32,
    bg: u32,
    pub input_buf: [u8; 256],
    pub input_len: usize,
    env: Vec<(String, String)>,
    ansi_esc_pending: bool,
    ansi_csi_mode: bool,
    ansi_osc_mode: bool,
    ansi_osc_esc_pending: bool,
    ansi_seq: [u8; ANSI_MAX_SEQ_LEN],
    ansi_seq_len: usize,
    ansi_saved_col: u32,
    ansi_saved_row: u32,
    scroll_top: u32,
    scroll_bottom: u32,
    insert_mode: bool,
    cursor_visible: bool,
    alt_screen: Option<AltScreenState>,
    last_printable: u8,
    cells: Vec<Cell>,
    // コマンドパスキャッシュ（最大16エントリ）
    cmd_cache: Vec<(String, String)>, // (cmd_name, full_path)
}

struct AltScreenState {
    cells: Vec<Cell>,
    col: u32,
    row: u32,
    fg: u32,
    bg: u32,
    scroll_top: u32,
    scroll_bottom: u32,
    insert_mode: bool,
}

#[derive(Clone, Copy)]
struct Cell {
    ch: u8,
    fg: u32,
    bg: u32,
}

impl Cell {
    const fn blank() -> Self {
        Self {
            ch: b' ',
            fg: DEFAULT_FG,
            bg: DEFAULT_BG,
        }
    }
}

#[allow(unused)]
impl Terminal {
    fn drain_pending_ipc_messages(&mut self, buf: &mut [u8]) -> bool {
        let mut wrote = false;
        while let Some((_, len)) = take_pending_message(buf) {
            if len == 0 || len > buf.len() {
                continue;
            }
            self.write_bytes(&buf[..len]);
            wrote = true;
        }
        wrote
    }

    fn load_env_file(&mut self) {
        let data = match read_file(ENV_FILE_PATH, ENV_FILE_MAX_SIZE)
        {
            Some(d) => d,
            None => return,
        };
        let text = match core::str::from_utf8(&data) {
            Ok(t) => t,
            Err(_) => return,
        };

        for raw_line in text.lines() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(eq) = line.find('=') {
                let key = line[..eq].trim();
                let val = line[eq + 1..].trim();
                if !key.is_empty() {
                    self.set_env(key, val);
                }
            }
        }
    }

    fn command_exists(&self, path: &str) -> bool {
        // stat syscall 未実装のため open/close で存在確認
        // 1回のopen試行で十分
        let fd = swiftlib::io::open(path, io::O_RDONLY);
        if fd >= 0 {
            swiftlib::io::close(fd as u64);
            return true;
        }
        false
    }

    fn should_try_busybox_alias(cmd: &str) -> bool {
        matches!(cmd, "ls" | "cat")
    }

    fn busybox_fallback_in_path(&mut self) -> Option<String> {
        // busybox専用キャッシュキー
        let cache_key = "__busybox__";
        if let Some(cached) = self.cmd_cache.iter().find(|(c, _)| c == cache_key) {
            return Some(cached.1.clone());
        }

        let path_val = self.get_env("PATH").unwrap_or_default();
        for dir in path_val.split(':') {
            let dir = dir.trim();
            if dir.is_empty() {
                continue;
            }
            let candidate = format!("{}/busybox.elf", dir);
            if self.command_exists(&candidate) {
                // キャッシュに追加
                if self.cmd_cache.len() >= 16 {
                    self.cmd_cache.remove(0);
                }
                self.cmd_cache.push((cache_key.to_string(), candidate.clone()));
                return Some(candidate);
            }
        }
        None
    }

    pub fn new(fb_ptr: *mut u32, info: vga::FbInfo, font: Font) -> Self {
        let max_cols = info.width / FONT_WIDTH as u32;
        let max_rows = info.height / FONT_HEIGHT as u32;
        let mut env = Vec::new();
        env.push(("PATH".to_string(), "/Binaries:/Applications".to_string()));
        let mut term = Terminal {
            fb_ptr,
            width: info.width,
            height: info.height,
            stride: info.stride,
            col: 0,
            row: 0,
            max_cols,
            max_rows,
            font,
            fg: DEFAULT_FG,
            bg: DEFAULT_BG,
            input_buf: [0u8; 256],
            input_len: 0,
            env,
            ansi_esc_pending: false,
            ansi_csi_mode: false,
            ansi_osc_mode: false,
            ansi_osc_esc_pending: false,
            ansi_seq: [0; ANSI_MAX_SEQ_LEN],
            ansi_seq_len: 0,
            ansi_saved_col: 0,
            ansi_saved_row: 0,
            scroll_top: 0,
            scroll_bottom: max_rows.saturating_sub(1),
            insert_mode: false,
            cursor_visible: true,
            alt_screen: None,
            last_printable: b' ',
            cells: vec![Cell::blank(); (max_cols as usize).saturating_mul(max_rows as usize)],
            cmd_cache: Vec::new(),
        };
        term.load_env_file();
        term
    }

    fn get_env(&self, key: &str) -> Option<String> {
        self.env.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    fn set_env(&mut self, key: &str, val: &str) {
        if let Some(entry) = self.env.iter_mut().find(|(k, _)| k == key) {
            entry.1 = val.to_string();
        } else {
            self.env.push((key.to_string(), val.to_string()));
        }
    }

    /// PATH の各ディレクトリでコマンドを探す
    /// `cmd` が `.elf` で終わる場合はそのまま、そうでなければ `.elf` を付けて検索する
    fn find_in_path(&mut self, cmd: &str) -> Option<String> {
        // キャッシュを確認
        if let Some(cached) = self.cmd_cache.iter().find(|(c, _)| c == cmd) {
            return Some(cached.1.clone());
        }

        let path_val = self.get_env("PATH").unwrap_or_default();
        let filename = if cmd.ends_with(".elf") {
            cmd.to_string()
        } else {
            format!("{}.elf", cmd)
        };
        for dir in path_val.split(':') {
            let dir = dir.trim();
            if dir.is_empty() {
                continue;
            }
            let candidate = format!("{}/{}", dir, filename);
            // stat syscall 未実装のため open/close で存在確認
            if self.command_exists(&candidate) {
                // キャッシュに追加（最大16エントリ）
                if self.cmd_cache.len() >= 16 {
                    self.cmd_cache.remove(0);
                }
                self.cmd_cache.push((cmd.to_string(), candidate.clone()));
                return Some(candidate);
            }
            if !cmd.ends_with(".elf") {
                let app_candidate = format!("{}/{}.app/entry.elf", dir, cmd);
                if self.command_exists(&app_candidate) {
                    if self.cmd_cache.len() >= 16 {
                        self.cmd_cache.remove(0);
                    }
                    self.cmd_cache
                        .push((cmd.to_string(), app_candidate.clone()));
                    return Some(app_candidate);
                }
            }
        }
        None
    }

    fn put_pixel(&self, x: u32, y: u32, color: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = (y * self.stride + x) as usize;
        // GOP 実装によっては上位8bitをアルファとして扱うため、常に不透明で書き込む。
        let opaque = color | 0xFF00_0000;
        unsafe {
            self.fb_ptr.add(offset).write_volatile(opaque);
        }
    }

    fn cell_index(&self, col: u32, row: u32) -> Option<usize> {
        if col >= self.max_cols || row >= self.max_rows {
            return None;
        }
        Some((row as usize) * (self.max_cols as usize) + (col as usize))
    }

    fn draw_char_pixels(&self, ch: u8, col: u32, row: u32, fg: u32, bg: u32) {
        let glyph = *self.font.glyph(ch);
        let x0 = col * FONT_WIDTH as u32;
        let y0 = row * FONT_HEIGHT as u32;
        for (r, &bits) in glyph.iter().enumerate() {
            let y = y0 + r as u32;
            if y >= self.height { break; }
            if x0 + FONT_WIDTH as u32 > self.width { break; }
            for c in 0..FONT_WIDTH {
                let on = (bits >> (7 - c)) & 1 != 0;
                let color = if on { fg } else { bg };
                self.put_pixel(x0 + c as u32, y, color);
            }
        }
    }

    fn set_cell(&mut self, col: u32, row: u32, cell: Cell) {
        if let Some(idx) = self.cell_index(col, row) {
            self.cells[idx] = cell;
            self.draw_char_pixels(cell.ch, col, row, cell.fg, cell.bg);
        }
    }

    fn get_cell(&self, col: u32, row: u32) -> Cell {
        match self.cell_index(col, row) {
            Some(idx) => self.cells[idx],
            None => Cell::blank(),
        }
    }

    fn redraw_from_cells(&self) {
        for row in 0..self.max_rows {
            for col in 0..self.max_cols {
                let c = self.get_cell(col, row);
                self.draw_char_pixels(c.ch, col, row, c.fg, c.bg);
            }
        }
    }

    pub fn clear_screen(&mut self) {
        let total = (self.height * self.stride) as usize;
        for i in 0..total {
            unsafe {
                self.fb_ptr.add(i).write_volatile(0);
            }
        }
        for cell in &mut self.cells {
            *cell = Cell::blank();
        }
        self.col = 0;
        self.row = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.max_rows.saturating_sub(1);
        self.insert_mode = false;
    }

    fn normalize_scroll_region(&mut self) {
        if self.max_rows == 0 {
            self.scroll_top = 0;
            self.scroll_bottom = 0;
            return;
        }
        if self.scroll_top >= self.max_rows {
            self.scroll_top = self.max_rows - 1;
        }
        if self.scroll_bottom >= self.max_rows {
            self.scroll_bottom = self.max_rows - 1;
        }
        if self.scroll_top > self.scroll_bottom {
            self.scroll_top = 0;
            self.scroll_bottom = self.max_rows - 1;
        }
    }

    fn scroll_region_up(&mut self, top: u32, bottom: u32, count: u32) {
        if self.max_rows == 0 || self.max_cols == 0 || top >= bottom {
            return;
        }
        let n = core::cmp::min(count.max(1), bottom - top + 1);
        for row in top..=bottom.saturating_sub(n) {
            for col in 0..self.max_cols {
                let src = self.get_cell(col, row + n);
                if let Some(idx) = self.cell_index(col, row) { self.cells[idx] = src; }
            }
        }
        for row in bottom.saturating_sub(n).saturating_add(1)..=bottom {
            for col in 0..self.max_cols {
                if let Some(idx) = self.cell_index(col, row) { self.cells[idx] = Cell::blank(); }
            }
        }
        self.redraw_from_cells();
    }

    fn scroll_region_down(&mut self, top: u32, bottom: u32, count: u32) {
        if self.max_rows == 0 || self.max_cols == 0 || top >= bottom {
            return;
        }
        let n = core::cmp::min(count.max(1), bottom - top + 1);
        let mut row = bottom;
        while row >= top + n {
            for col in 0..self.max_cols {
                let src = self.get_cell(col, row - n);
                if let Some(idx) = self.cell_index(col, row) { self.cells[idx] = src; }
            }
            if row == 0 { break; }
            row -= 1;
        }
        for row in top..top + n {
            for col in 0..self.max_cols {
                if let Some(idx) = self.cell_index(col, row) { self.cells[idx] = Cell::blank(); }
            }
        }
        self.redraw_from_cells();
    }

    fn scroll_up(&mut self) {
        if self.max_rows == 0 {
            return;
        }
        self.scroll_region_up(self.scroll_top, self.scroll_bottom, 1);
    }

    /// 互換性のために残す（シャドウバッファ廃止により no-op）
    pub fn flush(&mut self) {}

    fn index(&mut self) {
        self.normalize_scroll_region();
        if self.max_rows == 0 {
            return;
        }
        if self.row == self.scroll_bottom {
            self.scroll_region_up(self.scroll_top, self.scroll_bottom, 1);
        } else if self.row + 1 < self.max_rows {
            self.row += 1;
        }
    }

    fn reverse_index(&mut self) {
        self.normalize_scroll_region();
        if self.max_rows == 0 {
            return;
        }
        if self.row == self.scroll_top {
            self.scroll_region_down(self.scroll_top, self.scroll_bottom, 1);
        } else {
            self.row = self.row.saturating_sub(1);
        }
    }

    fn new_line(&mut self) {
        self.col = 0;
        self.index();
    }

    pub fn erase_previous_cell(&mut self) {
        if self.max_cols == 0 || self.max_rows == 0 {
            return;
        }
        if self.col > 0 {
            self.col -= 1;
            self.set_cell(
                self.col,
                self.row,
                Cell {
                    ch: b' ',
                    fg: self.fg,
                    bg: self.bg,
                },
            );
        }
    }

    fn set_scroll_region(&mut self, top: u32, bottom: u32) {
        if self.max_rows == 0 {
            self.scroll_top = 0;
            self.scroll_bottom = 0;
            return;
        }
        if top >= self.max_rows || bottom >= self.max_rows || top >= bottom {
            self.scroll_top = 0;
            self.scroll_bottom = self.max_rows - 1;
        } else {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        }
        self.col = 0;
        self.row = self.scroll_top;
    }

    fn enter_alt_screen(&mut self) {
        if self.alt_screen.is_some() {
            return;
        }
        let saved = AltScreenState {
            cells: self.cells.clone(),
            col: self.col,
            row: self.row,
            fg: self.fg,
            bg: self.bg,
            scroll_top: self.scroll_top,
            scroll_bottom: self.scroll_bottom,
            insert_mode: self.insert_mode,
        };
        self.alt_screen = Some(saved);
        for cell in &mut self.cells {
            *cell = Cell::blank();
        }
        self.col = 0;
        self.row = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.max_rows.saturating_sub(1);
        self.insert_mode = false;
        self.redraw_from_cells();
    }

    fn leave_alt_screen(&mut self) {
        if let Some(saved) = self.alt_screen.take() {
            self.cells = saved.cells;
            self.col = core::cmp::min(saved.col, self.max_cols.saturating_sub(1));
            self.row = core::cmp::min(saved.row, self.max_rows.saturating_sub(1));
            self.fg = saved.fg;
            self.bg = saved.bg;
            self.scroll_top = saved.scroll_top;
            self.scroll_bottom = saved.scroll_bottom;
            self.insert_mode = saved.insert_mode;
            self.normalize_scroll_region();
            self.redraw_from_cells();
        }
    }

    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            b'\r' => { self.col = 0; }
            b'\t' => {
                let next_tab = ((self.col / 8) + 1) * 8;
                while self.col < core::cmp::min(next_tab, self.max_cols) {
                    self.set_cell(
                        self.col,
                        self.row,
                        Cell {
                            ch: b' ',
                            fg: self.fg,
                            bg: self.bg,
                        },
                    );
                    self.col += 1;
                }
            }
            0x08 => { // Backspace
                if self.col > 0 {
                    self.col -= 1;
                    self.set_cell(
                        self.col,
                        self.row,
                        Cell {
                            ch: b' ',
                            fg: self.fg,
                            bg: self.bg,
                        },
                    );
                }
            }
            _ => {
                if !(0x20..=0x7E).contains(&byte) {
                    return;
                }
                if self.col >= self.max_cols {
                    self.new_line();
                }
                if self.insert_mode {
                    self.insert_blank_chars(1);
                }
                self.set_cell(
                    self.col,
                    self.row,
                    Cell {
                        ch: byte,
                        fg: self.fg,
                        bg: self.bg,
                    },
                );
                self.col += 1;
                self.last_printable = byte;
            }
        }
    }

    fn ansi_color(index: u16, bright: bool) -> Option<u32> {
        let i = index as usize;
        if i >= ANSI_COLOR_NORMAL.len() {
            return None;
        }
        if bright {
            Some(ANSI_COLOR_BRIGHT[i])
        } else {
            Some(ANSI_COLOR_NORMAL[i])
        }
    }

    fn apply_sgr_code(&mut self, code: u16) {
        match code {
            0 => {
                self.fg = DEFAULT_FG;
                self.bg = DEFAULT_BG;
            }
            30..=37 => {
                if let Some(color) = Self::ansi_color(code - 30, false) {
                    self.fg = color;
                }
            }
            90..=97 => {
                if let Some(color) = Self::ansi_color(code - 90, true) {
                    self.fg = color;
                }
            }
            39 => self.fg = DEFAULT_FG,
            40..=47 => {
                if let Some(color) = Self::ansi_color(code - 40, false) {
                    self.bg = color;
                }
            }
            100..=107 => {
                if let Some(color) = Self::ansi_color(code - 100, true) {
                    self.bg = color;
                }
            }
            49 => self.bg = DEFAULT_BG,
            _ => {}
        }
    }

    fn parse_ascii_u16(bytes: &[u8]) -> Option<u16> {
        let mut value = 0u16;
        for &b in bytes {
            if !b.is_ascii_digit() {
                return None;
            }
            value = value.saturating_mul(10).saturating_add((b - b'0') as u16);
        }
        Some(value)
    }

    fn apply_sgr_sequence(&mut self) {
        let params = self.csi_params_without_prefix();
        if params.is_empty() {
            self.apply_sgr_code(0);
            return;
        }
        for code in params {
            self.apply_sgr_code(code);
        }
    }

    fn reset_ansi_parser(&mut self) {
        self.ansi_esc_pending = false;
        self.ansi_csi_mode = false;
        self.ansi_osc_mode = false;
        self.ansi_osc_esc_pending = false;
        self.ansi_seq_len = 0;
    }

    fn csi_private_prefix(&self) -> Option<u8> {
        if self.ansi_seq_len == 0 {
            return None;
        }
        match self.ansi_seq[0] {
            b'?' | b'>' | b'!' => Some(self.ansi_seq[0]),
            _ => None,
        }
    }

    fn csi_params_from(&self, start_at: usize) -> Vec<u16> {
        let mut params = Vec::new();
        if start_at > self.ansi_seq_len {
            return params;
        }
        let mut start = start_at;
        let mut i = start_at;
        while i <= self.ansi_seq_len {
            if i == self.ansi_seq_len || self.ansi_seq[i] == b';' {
                if i == start {
                    params.push(0);
                } else if let Some(v) = Self::parse_ascii_u16(&self.ansi_seq[start..i]) {
                    params.push(v);
                }
                start = i + 1;
            }
            i += 1;
        }
        params
    }

    fn csi_params_without_prefix(&self) -> Vec<u16> {
        let start = if self.csi_private_prefix().is_some() { 1 } else { 0 };
        self.csi_params_from(start)
    }

    fn erase_cell(&mut self, col: u32, row: u32) {
        self.set_cell(
            col,
            row,
            Cell {
                ch: b' ',
                fg: self.fg,
                bg: self.bg,
            },
        );
    }

    fn erase_line_range(&mut self, row: u32, start_col: u32, end_col: u32) {
        if row >= self.max_rows {
            return;
        }
        let end = core::cmp::min(end_col, self.max_cols);
        let mut col = core::cmp::min(start_col, end);
        while col < end {
            self.erase_cell(col, row);
            col += 1;
        }
    }

    fn erase_screen_range(&mut self, start_row: u32, start_col: u32, end_row: u32, end_col: u32) {
        if self.max_rows == 0 || self.max_cols == 0 {
            return;
        }
        let mut row = start_row;
        while row < end_row && row < self.max_rows {
            let col_begin = if row == start_row { start_col } else { 0 };
            let col_end = if row + 1 == end_row {
                core::cmp::min(end_col, self.max_cols)
            } else {
                self.max_cols
            };
            self.erase_line_range(row, col_begin, col_end);
            row += 1;
        }
    }

    fn insert_blank_chars(&mut self, mut count: u32) {
        if self.row >= self.max_rows || self.col >= self.max_cols || count == 0 {
            return;
        }
        count = core::cmp::min(count, self.max_cols - self.col);
        let row = self.row;
        let start = self.col;
        let end = self.max_cols;
        let mut c = end;
        while c > start + count {
            let src = c - count - 1;
            let dst = c - 1;
            let moved = self.get_cell(src, row);
            self.set_cell(dst, row, moved);
            c -= 1;
        }
        self.erase_line_range(row, start, start + count);
    }

    fn delete_chars(&mut self, mut count: u32) {
        if self.row >= self.max_rows || self.col >= self.max_cols || count == 0 {
            return;
        }
        count = core::cmp::min(count, self.max_cols - self.col);
        let row = self.row;
        let start = self.col;
        let end = self.max_cols;
        let mut c = start;
        while c + count < end {
            let src_cell = self.get_cell(c + count, row);
            self.set_cell(c, row, src_cell);
            c += 1;
        }
        self.erase_line_range(row, end - count, end);
    }

    fn insert_blank_lines(&mut self, mut count: u32) {
        self.normalize_scroll_region();
        if self.row >= self.max_rows || count == 0 || self.row < self.scroll_top || self.row > self.scroll_bottom {
            return;
        }
        let start = self.row;
        let end = self.scroll_bottom + 1;
        count = core::cmp::min(count, end - start);
        let mut r = end;
        while r > start + count {
            let src = r - count - 1;
            let dst = r - 1;
            for col in 0..self.max_cols {
                let c = self.get_cell(col, src);
                self.set_cell(col, dst, c);
            }
            r -= 1;
        }
        for rr in start..start + count {
            self.erase_line_range(rr, 0, self.max_cols);
        }
    }

    fn delete_lines(&mut self, mut count: u32) {
        self.normalize_scroll_region();
        if self.row >= self.max_rows || count == 0 || self.row < self.scroll_top || self.row > self.scroll_bottom {
            return;
        }
        count = core::cmp::min(count, self.scroll_bottom - self.row + 1);
        let start = self.row;
        let end = self.scroll_bottom + 1;
        let mut r = start;
        while r + count < end {
            let src = r + count;
            for col in 0..self.max_cols {
                let c = self.get_cell(col, src);
                self.set_cell(col, r, c);
            }
            r += 1;
        }
        for rr in end - count..end {
            self.erase_line_range(rr, 0, self.max_cols);
        }
    }

    fn erase_chars(&mut self, mut count: u32) {
        if self.row >= self.max_rows || self.col >= self.max_cols || count == 0 {
            return;
        }
        count = core::cmp::min(count, self.max_cols - self.col);
        self.erase_line_range(self.row, self.col, self.col + count);
    }

    fn read_cell(&self, col: u32, row: u32) -> u8 {
        self.get_cell(col, row).ch
    }

    fn handle_csi_sequence(&mut self, final_byte: u8) {
        let private_prefix = self.csi_private_prefix();
        let params = self.csi_params_without_prefix();
        let p = |idx: usize, default: u16| -> u16 {
            let v = params.get(idx).copied().unwrap_or(default);
            if v == 0 {
                default
            } else {
                v
            }
        };

        let mut apply_mode = |mode: u16, enabled: bool| {
            match (private_prefix, mode) {
                (Some(b'?'), 25) => {
                    self.cursor_visible = enabled;
                }
                (Some(b'?'), 47) | (Some(b'?'), 1047) | (Some(b'?'), 1049) => {
                    if enabled {
                        self.enter_alt_screen();
                    } else {
                        self.leave_alt_screen();
                    }
                }
                (None, 4) => {
                    self.insert_mode = enabled;
                }
                _ => {}
            }
        };

        match final_byte {
            b'A' => {
                let n = p(0, 1) as u32;
                self.row = self.row.saturating_sub(n);
            }
            b'B' => {
                let n = p(0, 1) as u32;
                self.row = core::cmp::min(self.row.saturating_add(n), self.max_rows.saturating_sub(1));
            }
            b'C' => {
                let n = p(0, 1) as u32;
                self.col = core::cmp::min(self.col.saturating_add(n), self.max_cols.saturating_sub(1));
            }
            b'D' => {
                let n = p(0, 1) as u32;
                self.col = self.col.saturating_sub(n);
            }
            b'E' => {
                let n = p(0, 1) as u32;
                self.row = core::cmp::min(
                    self.row.saturating_add(n),
                    self.max_rows.saturating_sub(1),
                );
                self.col = 0;
            }
            b'F' => {
                let n = p(0, 1) as u32;
                self.row = self.row.saturating_sub(n);
                self.col = 0;
            }
            b'H' | b'f' => {
                let row = p(0, 1).saturating_sub(1) as u32;
                let col = p(1, 1).saturating_sub(1) as u32;
                self.row = core::cmp::min(row, self.max_rows.saturating_sub(1));
                self.col = core::cmp::min(col, self.max_cols.saturating_sub(1));
            }
            b'G' => {
                let col = p(0, 1).saturating_sub(1) as u32;
                self.col = core::cmp::min(col, self.max_cols.saturating_sub(1));
            }
            b'd' => {
                let row = p(0, 1).saturating_sub(1) as u32;
                self.row = core::cmp::min(row, self.max_rows.saturating_sub(1));
            }
            b'J' => {
                let mode = params.get(0).copied().unwrap_or(0);
                match mode {
                    0 => {
                        self.erase_screen_range(
                            self.row,
                            self.col,
                            self.max_rows,
                            self.max_cols,
                        );
                    }
                    1 => {
                        self.erase_screen_range(0, 0, self.row + 1, self.col + 1);
                    }
                    2 => {
                        self.erase_screen_range(0, 0, self.max_rows, self.max_cols);
                        self.col = 0;
                        self.row = 0;
                    }
                    _ => {}
                }
            }
            b'K' => {
                let mode = params.get(0).copied().unwrap_or(0);
                match mode {
                    0 => self.erase_line_range(self.row, self.col, self.max_cols),
                    1 => self.erase_line_range(self.row, 0, self.col + 1),
                    2 => self.erase_line_range(self.row, 0, self.max_cols),
                    _ => {}
                }
            }
            b's' => {
                self.ansi_saved_col = self.col;
                self.ansi_saved_row = self.row;
            }
            b'u' => {
                self.col = core::cmp::min(self.ansi_saved_col, self.max_cols.saturating_sub(1));
                self.row = core::cmp::min(self.ansi_saved_row, self.max_rows.saturating_sub(1));
            }
            b'@' => {
                let n = p(0, 1) as u32;
                self.insert_blank_chars(n);
            }
            b'P' => {
                let n = p(0, 1) as u32;
                self.delete_chars(n);
            }
            b'L' => {
                let n = p(0, 1) as u32;
                self.insert_blank_lines(n);
            }
            b'M' => {
                let n = p(0, 1) as u32;
                self.delete_lines(n);
            }
            b'X' => {
                let n = p(0, 1) as u32;
                self.erase_chars(n);
            }
            b'r' => {
                let top = p(0, 1).saturating_sub(1) as u32;
                let bottom = p(1, self.max_rows as u16).saturating_sub(1) as u32;
                self.set_scroll_region(top, bottom);
            }
            b'S' => {
                let n = p(0, 1) as u32;
                self.scroll_region_up(self.scroll_top, self.scroll_bottom, n);
            }
            b'T' => {
                let n = p(0, 1) as u32;
                self.scroll_region_down(self.scroll_top, self.scroll_bottom, n);
            }
            b'b' => {
                let n = p(0, 1) as usize;
                for _ in 0..n {
                    self.write_byte(self.last_printable);
                }
            }
            b'h' => {
                if params.is_empty() {
                    apply_mode(0, true);
                } else {
                    for &mode in &params {
                        apply_mode(mode, true);
                    }
                }
            }
            b'l' => {
                if params.is_empty() {
                    apply_mode(0, false);
                } else {
                    for &mode in &params {
                        apply_mode(mode, false);
                    }
                }
            }
            _ => {}
        }
    }

    fn write_output_byte(&mut self, byte: u8) {
        if self.ansi_osc_mode {
            if byte == 0x07 {
                self.reset_ansi_parser();
                return;
            }
            if self.ansi_osc_esc_pending {
                self.ansi_osc_esc_pending = false;
                if byte == b'\\' {
                    self.reset_ansi_parser();
                }
                return;
            }
            if byte == 0x1B {
                self.ansi_osc_esc_pending = true;
            }
            return;
        }

        if self.ansi_esc_pending {
            self.ansi_esc_pending = false;
            if byte == b'[' {
                self.ansi_csi_mode = true;
                self.ansi_seq_len = 0;
            } else if byte == b']' {
                self.ansi_osc_mode = true;
                self.ansi_osc_esc_pending = false;
            } else if byte == b'7' {
                self.ansi_saved_col = self.col;
                self.ansi_saved_row = self.row;
            } else if byte == b'8' {
                self.col = core::cmp::min(self.ansi_saved_col, self.max_cols.saturating_sub(1));
                self.row = core::cmp::min(self.ansi_saved_row, self.max_rows.saturating_sub(1));
            } else if byte == b'D' {
                self.index();
            } else if byte == b'E' {
                self.new_line();
                self.col = 0;
            } else if byte == b'M' {
                self.reverse_index();
            } else if byte == b'c' {
                self.clear_screen();
                self.fg = DEFAULT_FG;
                self.bg = DEFAULT_BG;
                self.cursor_visible = true;
                self.alt_screen = None;
                self.last_printable = b' ';
            } else if byte == 0x1B {
                self.ansi_esc_pending = true;
            }
            return;
        }

        if self.ansi_csi_mode {
            if byte == b'm' {
                self.apply_sgr_sequence();
                self.reset_ansi_parser();
                return;
            }

            if (0x20..=0x3F).contains(&byte) {
                if self.ansi_seq_len < self.ansi_seq.len() {
                    self.ansi_seq[self.ansi_seq_len] = byte;
                    self.ansi_seq_len += 1;
                } else {
                    self.reset_ansi_parser();
                }
                return;
            }

            if (0x40..=0x7E).contains(&byte) {
                self.handle_csi_sequence(byte);
                self.reset_ansi_parser();
                return;
            }

            self.reset_ansi_parser();
            return;
        }

        if byte == 0x1B {
            self.ansi_esc_pending = true;
            return;
        }

        self.write_byte(byte);
    }

    pub fn write_str(&mut self, s: &str) {
        for b in s.bytes() {
            self.write_output_byte(b);
        }
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_output_byte(b);
        }
    }

    fn write_num(&mut self, mut n: u64) {
        if n == 0 {
            self.write_byte(b'0');
            return;
        }
        let mut buf = [0u8; 20];
        let mut i = 20;
        while n > 0 {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
        }
        for &b in &buf[i..] {
            self.write_byte(b);
        }
    }

    pub fn prompt(&mut self) {
        let mut cwd_buf = [0u8; 256];
        let cwd = fs::getcwd(&mut cwd_buf).unwrap_or("/");
        self.fg = 0x00FF_88FF; // 紫
        self.write_str(cwd);
        self.write_str(" mochi> ");
        self.fg = 0x00FF_FFFF; // シアン
    }

    pub fn size_chars(&self) -> (u16, u16) {
        (self.max_cols as u16, self.max_rows as u16)
    }

    /// 子プロセスのIPC出力を受け取りながら終了を待つ
    fn drain_child_output(&mut self, pid: u64) {
        let mut buf = Box::new([0u8; IPC_MSG_MAX]);
        loop {
            let mut wrote = false;
            if self.drain_pending_ipc_messages(&mut *buf) {
                wrote = true;
            }
            loop {
                let (_, len2) = ipc::ipc_recv(&mut *buf);
                if len2 == 0 || len2 as usize > buf.len() {
                    break;
                }
                self.write_bytes(&buf[..len2 as usize]);
                wrote = true;
            }
            if wrote {
                self.flush();
            }
            let child_finished = match task::wait_nonblocking_status(pid as i64) {
                task::WaitNonblockingStatus::Exited(_) => true,
                task::WaitNonblockingStatus::Running => false,
                task::WaitNonblockingStatus::NoChild => true,
                task::WaitNonblockingStatus::Error(_) => true,
            };
            if child_finished {
                break;
            }

            // メッセージが届くまでスリープして待機（ビジーウェイトしない）
            let (_, len) = ipc::ipc_recv_wait(&mut *buf);
            if len > 0 && len as usize <= buf.len() {
                self.write_bytes(&buf[..len as usize]);
                // 続きのメッセージをノンブロッキングで掃き出す
                loop {
                    let (_, len2) = ipc::ipc_recv(&mut *buf);
                    if len2 == 0 || len2 as usize > buf.len() {
                        break;
                    }
                    self.write_bytes(&buf[..len2 as usize]);
                }
                // バッチ分まとめてフラッシュ
                self.flush();
            }
            // 子プロセスが終了していれば抜ける（exit 通知で起床した場合もここで検知）
            let child_finished = match task::wait_nonblocking_status(pid as i64) {
                task::WaitNonblockingStatus::Exited(_) => true,
                task::WaitNonblockingStatus::Running => false,
                task::WaitNonblockingStatus::NoChild => true,
                task::WaitNonblockingStatus::Error(_) => true,
            };
            if child_finished {
                break;
            }
        }
        // 終了後に残ったメッセージを念のため掃き出す
        let mut wrote = false;
        if self.drain_pending_ipc_messages(&mut *buf) {
            wrote = true;
        }
        loop {
            let (_, len) = ipc::ipc_recv(&mut *buf);
            if len == 0 || len as usize > buf.len() {
                break;
            }
            self.write_bytes(&buf[..len as usize]);
            wrote = true;
        }
        if wrote {
            self.flush();
        }
    }

    // ================================================================
    // ネイティブビルトインコマンド群
    // BusyBox を介さずシェル内で直接実装し、ELF ロード/IPC コマンド実行の
    // オーバーヘッドを回避する。
    // ================================================================

    /// 共通: ファイル内容を取得
    fn load_file_bytes(&self, path: &str, limit: usize) -> Option<Vec<u8>> {
        let abs = resolve_path(path);
        read_file_from_fs(&abs, limit)
    }

    /// 共通: ファイル内容をテキストとして書き出す
    fn write_file_text(&mut self, data: &[u8]) {
        if let Ok(text) = core::str::from_utf8(data) {
            self.write_str(text);
            if !text.ends_with('\n') {
                self.write_byte(b'\n');
            }
        } else {
            // バイナリは可読文字のみ表示
            for &b in data {
                if b == b'\n' || b == b'\t' || (0x20..0x7F).contains(&b) {
                    self.write_byte(b);
                } else {
                    self.write_byte(b'.');
                }
            }
            self.write_byte(b'\n');
        }
    }

    /// ls [-l] [path...]
    fn builtin_ls(&mut self, args: &[String]) {
        let mut long = false;
        let mut targets: Vec<&str> = Vec::new();
        for a in args {
            if a == "-l" || a == "-la" || a == "-al" {
                long = true;
            } else if a.starts_with('-') {
                // 未知オプションは無視
            } else {
                targets.push(a.as_str());
            }
        }
        if targets.is_empty() {
            targets.push(".");
        }

        let multi = targets.len() > 1;
        for (i, t) in targets.iter().enumerate() {
            if multi {
                if i > 0 {
                    self.write_byte(b'\n');
                }
                self.write_str(t);
                self.write_str(":\n");
            }
            let abs = resolve_path(t);

            // まず stat してファイル/ディレクトリ判定
            match stat_via_fs_service(&abs) {
                Some((mode, size)) => {
                    let is_dir = (mode & S_IFMT) == S_IFDIR;
                    if !is_dir {
                        // 単一ファイル指定
                        if long {
                            self.ls_print_long_line(t, mode, size);
                        } else {
                            self.write_str(t);
                            self.write_byte(b'\n');
                        }
                        continue;
                    }
                }
                None => {
                    self.write_str("ls: cannot access '");
                    self.write_str(t);
                    self.write_str("': No such file or directory\n");
                    continue;
                }
            }

            // ディレクトリを列挙
            let entries = match readdir_all_via_fs_service(&abs) {
                Some(e) => e,
                None => {
                    self.write_str("ls: cannot read directory '");
                    self.write_str(t);
                    self.write_str("'\n");
                    continue;
                }
            };

            // ソート（名前順）
            let mut sorted = entries;
            sorted.sort();

            for name in &sorted {
                let child_path = if abs == "/" {
                    format!("/{}", name)
                } else {
                    format!("{}/{}", abs, name)
                };
                if long {
                    match stat_via_fs_service(&child_path) {
                        Some((mode, size)) => {
                            self.ls_print_long_line(name, mode, size);
                        }
                        None => {
                            self.write_str("? ");
                            self.write_str(name);
                            self.write_byte(b'\n');
                        }
                    }
                } else {
                    // 色分け: ディレクトリは青、その他は白
                    let mode = stat_via_fs_service(&child_path).map(|(m, _)| m).unwrap_or(0);
                    let is_dir = (mode & S_IFMT) == S_IFDIR;
                    if is_dir {
                        self.fg = 0x005599FF;
                    }
                    self.write_str(name);
                    if is_dir {
                        self.write_byte(b'/');
                        self.fg = DEFAULT_FG;
                    }
                    self.write_byte(b'\n');
                }
            }
        }
    }

    fn ls_print_long_line(&mut self, name: &str, mode: u64, size: u64) {
        let is_dir = (mode & S_IFMT) == S_IFDIR;
        // type bit
        self.write_byte(if is_dir { b'd' } else { b'-' });
        // rwx for user/group/other
        let perm = mode & 0o777;
        let bits = [
            (perm >> 8) & 1, (perm >> 7) & 1, (perm >> 6) & 1,
            (perm >> 5) & 1, (perm >> 4) & 1, (perm >> 3) & 1,
            (perm >> 2) & 1, (perm >> 1) & 1, perm & 1,
        ];
        let chars = [b'r', b'w', b'x', b'r', b'w', b'x', b'r', b'w', b'x'];
        for i in 0..9 {
            self.write_byte(if bits[i] != 0 { chars[i] } else { b'-' });
        }
        self.write_byte(b' ');
        // size（右詰め 10 桁）
        let size_str = format!("{}", size);
        let pad = 10usize.saturating_sub(size_str.len());
        for _ in 0..pad {
            self.write_byte(b' ');
        }
        self.write_str(&size_str);
        self.write_byte(b' ');
        // name (ディレクトリは色付け)
        if is_dir {
            self.fg = 0x005599FF;
        }
        self.write_str(name);
        if is_dir {
            self.write_byte(b'/');
            self.fg = DEFAULT_FG;
        }
        self.write_byte(b'\n');
    }

    /// cat file...
    fn builtin_cat(&mut self, args: &[String]) {
        if args.is_empty() {
            self.write_str("usage: cat <file>...\n");
            return;
        }
        for arg in args {
            match self.load_file_bytes(arg, 1024 * 1024) {
                Some(data) => self.write_file_text(&data),
                None => {
                    self.write_str("cat: ");
                    self.write_str(arg);
                    self.write_str(": No such file\n");
                }
            }
        }
    }

    /// echo [-n] args...
    fn builtin_echo(&mut self, args: &[String]) {
        let mut trailing_newline = true;
        let mut start = 0;
        if let Some(first) = args.first() {
            if first == "-n" {
                trailing_newline = false;
                start = 1;
            }
        }
        for (i, a) in args[start..].iter().enumerate() {
            if i > 0 {
                self.write_byte(b' ');
            }
            self.write_str(a);
        }
        if trailing_newline {
            self.write_byte(b'\n');
        }
    }

    /// pwd
    fn builtin_pwd(&mut self) {
        let mut buf = [0u8; 256];
        let cwd = fs::getcwd(&mut buf).unwrap_or("/");
        self.write_str(cwd);
        self.write_byte(b'\n');
    }

    /// stat file...
    fn builtin_stat(&mut self, args: &[String]) {
        if args.is_empty() {
            self.write_str("usage: stat <file>...\n");
            return;
        }
        for arg in args {
            let abs = resolve_path(arg);
            match stat_via_fs_service(&abs) {
                Some((mode, size)) => {
                    let is_dir = (mode & S_IFMT) == S_IFDIR;
                    self.write_str("  File: ");
                    self.write_str(arg);
                    self.write_byte(b'\n');
                    self.write_str("  Type: ");
                    self.write_str(if is_dir { "directory" } else { "regular file" });
                    self.write_byte(b'\n');
                    self.write_str("  Size: ");
                    self.write_str(&format!("{}", size));
                    self.write_byte(b'\n');
                    self.write_str("  Mode: ");
                    self.write_str(&format!("{:o}", mode & 0o7777));
                    self.write_byte(b'\n');
                }
                None => {
                    self.write_str("stat: cannot stat '");
                    self.write_str(arg);
                    self.write_str("': No such file or directory\n");
                }
            }
        }
    }

    /// head [-n N] file...
    fn builtin_head(&mut self, args: &[String]) {
        let (n, files) = Self::parse_nflag(args, 10);
        if files.is_empty() {
            self.write_str("usage: head [-n N] <file>...\n");
            return;
        }
        let multi = files.len() > 1;
        for (i, arg) in files.iter().enumerate() {
            if multi {
                if i > 0 {
                    self.write_byte(b'\n');
                }
                self.write_str("==> ");
                self.write_str(arg);
                self.write_str(" <==\n");
            }
            match self.load_file_bytes(arg, 1024 * 1024) {
                Some(data) => {
                    if let Ok(text) = core::str::from_utf8(&data) {
                        for (idx, line) in text.lines().enumerate() {
                            if idx >= n {
                                break;
                            }
                            self.write_str(line);
                            self.write_byte(b'\n');
                        }
                    } else {
                        self.write_str("head: binary file\n");
                    }
                }
                None => {
                    self.write_str("head: ");
                    self.write_str(arg);
                    self.write_str(": No such file\n");
                }
            }
        }
    }

    /// tail [-n N] file...
    fn builtin_tail(&mut self, args: &[String]) {
        let (n, files) = Self::parse_nflag(args, 10);
        if files.is_empty() {
            self.write_str("usage: tail [-n N] <file>...\n");
            return;
        }
        let multi = files.len() > 1;
        for (i, arg) in files.iter().enumerate() {
            if multi {
                if i > 0 {
                    self.write_byte(b'\n');
                }
                self.write_str("==> ");
                self.write_str(arg);
                self.write_str(" <==\n");
            }
            match self.load_file_bytes(arg, 1024 * 1024) {
                Some(data) => {
                    if let Ok(text) = core::str::from_utf8(&data) {
                        let lines: Vec<&str> = text.lines().collect();
                        let start = lines.len().saturating_sub(n);
                        for line in &lines[start..] {
                            self.write_str(line);
                            self.write_byte(b'\n');
                        }
                    } else {
                        self.write_str("tail: binary file\n");
                    }
                }
                None => {
                    self.write_str("tail: ");
                    self.write_str(arg);
                    self.write_str(": No such file\n");
                }
            }
        }
    }

    /// -n フラグを処理して (count, files) を返す
    fn parse_nflag(args: &[String], default: usize) -> (usize, Vec<String>) {
        let mut n = default;
        let mut files: Vec<String> = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a == "-n" {
                if i + 1 < args.len() {
                    if let Ok(v) = args[i + 1].parse::<usize>() {
                        n = v;
                    }
                    i += 2;
                    continue;
                }
            } else if let Some(rest) = a.strip_prefix("-n") {
                if let Ok(v) = rest.parse::<usize>() {
                    n = v;
                    i += 1;
                    continue;
                }
            }
            files.push(a.clone());
            i += 1;
        }
        (n, files)
    }

    /// wc [-lwc] file...
    fn builtin_wc(&mut self, args: &[String]) {
        let mut show_lines = false;
        let mut show_words = false;
        let mut show_bytes = false;
        let mut files: Vec<&str> = Vec::new();
        for a in args {
            if let Some(flags) = a.strip_prefix('-') {
                for ch in flags.chars() {
                    match ch {
                        'l' => show_lines = true,
                        'w' => show_words = true,
                        'c' => show_bytes = true,
                        _ => {}
                    }
                }
            } else {
                files.push(a.as_str());
            }
        }
        if !show_lines && !show_words && !show_bytes {
            show_lines = true;
            show_words = true;
            show_bytes = true;
        }
        if files.is_empty() {
            self.write_str("usage: wc [-lwc] <file>...\n");
            return;
        }
        for arg in &files {
            match self.load_file_bytes(arg, 1024 * 1024) {
                Some(data) => {
                    let bytes = data.len();
                    let text = core::str::from_utf8(&data).unwrap_or("");
                    let lines = text.lines().count();
                    let words = text.split_whitespace().count();
                    let mut first = true;
                    if show_lines {
                        self.write_str(&format!("{:>7}", lines));
                        first = false;
                    }
                    if show_words {
                        if !first { self.write_byte(b' '); }
                        self.write_str(&format!("{:>7}", words));
                        first = false;
                    }
                    if show_bytes {
                        if !first { self.write_byte(b' '); }
                        self.write_str(&format!("{:>7}", bytes));
                    }
                    self.write_byte(b' ');
                    self.write_str(arg);
                    self.write_byte(b'\n');
                }
                None => {
                    self.write_str("wc: ");
                    self.write_str(arg);
                    self.write_str(": No such file\n");
                }
            }
        }
    }

    /// grep pattern file...
    fn builtin_grep(&mut self, args: &[String]) {
        if args.len() < 2 {
            self.write_str("usage: grep <pattern> <file>...\n");
            return;
        }
        let pattern = args[0].as_str();
        let files = &args[1..];
        let multi = files.len() > 1;
        for arg in files {
            match self.load_file_bytes(arg, 1024 * 1024) {
                Some(data) => {
                    if let Ok(text) = core::str::from_utf8(&data) {
                        for line in text.lines() {
                            if line.contains(pattern) {
                                if multi {
                                    self.write_str(arg);
                                    self.write_byte(b':');
                                }
                                self.write_str(line);
                                self.write_byte(b'\n');
                            }
                        }
                    }
                }
                None => {
                    self.write_str("grep: ");
                    self.write_str(arg);
                    self.write_str(": No such file\n");
                }
            }
        }
    }

    /// which cmd...
    fn builtin_which(&mut self, args: &[String]) {
        if args.is_empty() {
            self.write_str("usage: which <cmd>...\n");
            return;
        }
        for arg in args {
            let name = arg.clone();
            if let Some(path) = self.find_in_path(&name) {
                self.write_str(&path);
                self.write_byte(b'\n');
            } else {
                self.write_str(arg);
                self.write_str(": not found\n");
            }
        }
    }

    /// about <app>
    fn builtin_about(&mut self, args: &[String]) {
        if args.len() != 1 {
            self.write_str("usage: about <app>\n");
            return;
        }
        let raw_name = args[0].trim();
        if raw_name.is_empty() {
            self.write_str("usage: about <app>\n");
            return;
        }
        let app_name = raw_name.strip_suffix(".app").unwrap_or(raw_name);
        let about_path = format!("/Applications/{}.app/about.toml", app_name);
        match self.load_file_bytes(&about_path, 16 * 1024) {
            Some(data) => {
                if let Ok(text) = core::str::from_utf8(&data) {
                    self.write_str(text);
                    if !text.ends_with('\n') {
                        self.write_byte(b'\n');
                    }
                } else {
                    self.write_str("about: about.toml is not valid UTF-8\n");
                }
            }
            None => {
                self.write_str("about: app not found: ");
                self.write_str(app_name);
                self.write_byte(b'\n');
            }
        }
    }

    /// env
    fn builtin_env(&mut self) {
        for (k, v) in self.env.clone().iter() {
            self.write_str(k);
            self.write_byte(b'=');
            self.write_str(v);
            self.write_byte(b'\n');
        }
    }

    /// basename path
    fn builtin_basename(&mut self, args: &[String]) {
        if args.is_empty() {
            self.write_str("usage: basename <path>\n");
            return;
        }
        let p = args[0].trim_end_matches('/');
        let base = match p.rfind('/') {
            Some(i) => &p[i + 1..],
            None => p,
        };
        let base = if base.is_empty() { "/" } else { base };
        self.write_str(base);
        self.write_byte(b'\n');
    }

    /// dirname path
    fn builtin_dirname(&mut self, args: &[String]) {
        if args.is_empty() {
            self.write_str("usage: dirname <path>\n");
            return;
        }
        let p = args[0].trim_end_matches('/');
        let dir = match p.rfind('/') {
            Some(0) => "/",
            Some(i) => &p[..i],
            None => ".",
        };
        self.write_str(dir);
        self.write_byte(b'\n');
    }

    fn parse_command_line(line: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        let mut quote: Option<u8> = None;

        for b in line.bytes() {
            match quote {
                Some(q) => {
                    if b == q {
                        quote = None;
                    } else {
                        current.push(b as char);
                    }
                }
                None => match b {
                    b'"' | b'\'' => quote = Some(b),
                    b' ' | b'\t' => {
                        if !current.is_empty() {
                            tokens.push(current);
                            current = String::new();
                        }
                    }
                    _ => current.push(b as char),
                },
            }
        }

        if !current.is_empty() {
            tokens.push(current);
        }

        tokens
    }

    pub fn handle_line(&mut self) {
        // バッファから文字列をコピーして借用を解放
        let mut tmp = [0u8; 256];
        let len = self.input_len;
        tmp[..len].copy_from_slice(&self.input_buf[..len]);
        let cmd_str: &str = core::str::from_utf8(&tmp[..len]).unwrap_or("").trim();

        let mut cmd_buf = [0u8; 256];
        let cmd_bytes = cmd_str.as_bytes();
        cmd_buf[..cmd_bytes.len()].copy_from_slice(cmd_bytes);
        let cmd_len = cmd_bytes.len();

        self.write_byte(b'\n');
        self.input_len = 0;

        let cmd = core::str::from_utf8(&cmd_buf[..cmd_len]).unwrap_or("");
        if cmd.is_empty() {
            return;
        }

        let tokens = Self::parse_command_line(cmd);
        if tokens.is_empty() {
            return;
        }

        let cmd_name = tokens[0].as_str();
        let args = &tokens[1..];
        let joined_args = if args.is_empty() {
            String::new()
        } else {
            args.join(" ")
        };

        match cmd_name {
            "help" => {
                self.write_str("mochiOS shell builtins:\n");
                self.write_str("  ls [-l] [path]     cat <file>...     echo [-n] args\n");
                self.write_str("  pwd                cd <dir>          stat <file>\n");
                self.write_str("  head [-n N] <f>    tail [-n N] <f>   wc [-lwc] <f>\n");
                self.write_str("  grep <pat> <f>     which <cmd>       env\n");
                self.write_str("  basename <p>       dirname <p>       export K=V\n");
                self.write_str("  clear              version           about <app>\n");
                self.write_str("  true / false\n");
                self.write_str("External binaries in $PATH are executed directly (no BusyBox).\n");
            }
            "clear" => {
                self.clear_screen();
            }
            "version" => {
                if let Some(data) = read_file_from_fs("/System/about.txt", 4096) {
                    if let Ok(text) = core::str::from_utf8(&data) {
                        self.write_str(text);
                        if !text.ends_with('\n') {
                            self.write_byte(b'\n');
                        }
                    } else {
                        self.write_str("Error: /System/about.txt is not valid UTF-8\n");
                    }
                } else {
                    self.write_str("mochiOS (about.txt not found)\n");
                }
            }
            "about" => self.builtin_about(args),
            "cd" => {
                let target = args.first().map(|s| s.as_str()).unwrap_or("/");
                let ret = fs::chdir(target);
                if ret != 0 {
                    self.write_str("cd: no such directory: ");
                    self.write_str(target);
                    self.write_byte(b'\n');
                }
            }
            "export" => {
                if let Some(eq) = joined_args.find('=') {
                    let key = joined_args[..eq].trim().to_string();
                    let val = joined_args[eq + 1..].trim().to_string();
                    self.set_env(&key, &val);
                } else {
                    self.write_str("usage: export VAR=VALUE\n");
                }
            }
            "ls"       => self.builtin_ls(args),
            "cat"      => self.builtin_cat(args),
            "echo"     => self.builtin_echo(args),
            "pwd"      => self.builtin_pwd(),
            "stat"     => self.builtin_stat(args),
            "head"     => self.builtin_head(args),
            "tail"     => self.builtin_tail(args),
            "wc"       => self.builtin_wc(args),
            "grep"     => self.builtin_grep(args),
            "which"    => self.builtin_which(args),
            "env"      => self.builtin_env(),
            "basename" => self.builtin_basename(args),
            "dirname"  => self.builtin_dirname(args),
            "true"     => {}
            "false"    => {}
            _ => {
                // PATH から直接実行（BusyBox フォールバック廃止）
                match self.find_in_path(cmd_name).map(|s| s.to_string()) {
                    Some(bin_path) => {
                        let arg_parts: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                        match exec_via_fs_service(&bin_path, &arg_parts) {
                            Ok(pid) => {
                                self.drain_child_output(pid);
                            }
                            Err(_) => {
                                self.write_str("exec failed: ");
                                self.write_str(&bin_path);
                                self.write_byte(b'\n');
                            }
                        }
                    }
                    None => {
                        self.write_str("command not found: ");
                        self.write_str(cmd_name);
                        self.write_byte(b'\n');
                    }
                }
            }
        }
    }
}
