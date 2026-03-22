use core::mem::size_of;
use swiftlib::{fs, io, ipc, task, vga};

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
const FS_DATA_MAX: usize = 560;

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
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FsResponse {
    status: i64,
    len: u64,
    data: [u8; FS_DATA_MAX],
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

fn fs_request(fs_tid: u64, req: &FsRequest) -> Result<FsResponse, ()> {
    let req_slice = unsafe {
        core::slice::from_raw_parts(&req as *const _ as *const u8, size_of::<FsRequest>())
    };
    if ipc::ipc_send(fs_tid, req_slice) != 0 {
        return Err(());
    }

    let mut resp_buf = [0u8; size_of::<FsResponse>()];
    loop {
        let (sender, len) = ipc::ipc_recv_wait(&mut resp_buf);
        if sender == 0 && len == 0 {
            continue;
        }
        if sender != fs_tid || (len as usize) < size_of::<FsResponse>() {
            continue;
        }
        let resp: FsResponse = unsafe {
            core::ptr::read_unaligned(resp_buf.as_ptr() as *const FsResponse)
        };
        return Ok(resp);
    }
}

fn open_via_fs_service(fs_tid: u64, path: &str) -> Result<u64, ()> {
    let path_field = encode_exec_path_and_args(path, &[]).ok_or(())?;
    let req = FsRequest {
        op: FsRequest::OP_OPEN,
        arg1: 0,
        arg2: 0,
        path: path_field,
    };
    let resp = fs_request(fs_tid, &req)?;
    if resp.status < 0 {
        return Err(());
    }
    Ok(resp.status as u64)
}

fn close_via_fs_service(fs_tid: u64, fd: u64) {
    let req = FsRequest {
        op: FsRequest::OP_CLOSE,
        arg1: fd,
        arg2: 0,
        path: [0; FS_PATH_MAX],
    };
    let _ = fs_request(fs_tid, &req);
}

fn read_file_via_fs_service(path: &str, max_size: usize) -> Option<Vec<u8>> {
    let fs_tid = task::find_process_by_name("fs.service")?;
    let fd = match open_via_fs_service(fs_tid, path) {
        Ok(fd) => fd,
        Err(()) => return None,
    };

    let mut out = Vec::new();
    while out.len() < max_size {
        let req_len = core::cmp::min(FS_DATA_MAX, max_size - out.len());
        if req_len == 0 {
            break;
        }

        let req = FsRequest {
            op: FsRequest::OP_READ,
            arg1: fd,
            arg2: req_len as u64,
            path: [0; FS_PATH_MAX],
        };
        let resp = match fs_request(fs_tid, &req) {
            Ok(r) => r,
            Err(()) => {
                close_via_fs_service(fs_tid, fd);
                return None;
            }
        };
        if resp.status < 0 {
            close_via_fs_service(fs_tid, fd);
            return None;
        }

        let n = core::cmp::min(resp.len as usize, FS_DATA_MAX);
        if n == 0 {
            break;
        }
        out.extend_from_slice(&resp.data[..n]);
    }
    close_via_fs_service(fs_tid, fd);
    Some(out)
}

fn exec_via_fs_service(path: &str, args: &[&str]) -> Result<u64, i64> {
    let fs_tid = task::find_process_by_name("fs.service").ok_or(-5)?;
    let path_field = encode_exec_path_and_args(path, args).ok_or(-22)?;
    let req = FsRequest {
        op: FsRequest::OP_EXEC,
        arg1: 0,
        arg2: 0,
        path: path_field,
    };
    let resp = fs_request(fs_tid, &req).map_err(|_| -5)?;
    if resp.status < 0 {
        return Err(resp.status);
    }
    Ok(resp.status as u64)
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
        let data = read_file(FONT_BIN_PATH, FONT_BIN_SIZE)?;
        if data.len() < FONT_BIN_SIZE {
            return None;
        }

        let mut glyphs = [[0u8; FONT_HEIGHT]; GLYPH_COUNT];
        for (i, glyph) in glyphs.iter_mut().enumerate() {
            let start = i * FONT_HEIGHT;
            glyph.copy_from_slice(&data[start..start + FONT_HEIGHT]);
        }
        Some(Font { glyphs })
    }

    fn load_from_bdf() -> Option<Self> {
        let data = read_file(FONT_BDF_PATH, FONT_BDF_MAX_SIZE)?;
        let mut glyphs = [[0u8; FONT_HEIGHT]; GLYPH_COUNT];
        parse_bdf(&data, &mut glyphs);
        Some(Font { glyphs })
    }

    /// `System/fonts/ter-u12b.bin` を優先し、失敗時はBDFを解析する
    pub fn load() -> Option<Self> {
        if let Some(font) = Self::load_from_binary() {
            return Some(font);
        }
        if let Some(font) = Self::load_from_bdf() {
            return Some(font);
        }
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
    ansi_seq: [u8; ANSI_MAX_SEQ_LEN],
    ansi_seq_len: usize,
}

#[allow(unused)]
impl Terminal {
    fn load_env_file(&mut self) {
        let data = match read_file_via_fs_service(ENV_FILE_PATH, ENV_FILE_MAX_SIZE) {
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
            } else {
                self.set_env("PATH", line);
            }
        }
    }

    fn command_exists(&self, path: &str) -> bool {
        if let Some(fs_tid) = task::find_process_by_name("fs.service") {
            if let Ok(fd) = open_via_fs_service(fs_tid, path) {
                close_via_fs_service(fs_tid, fd);
                return true;
            }
        }

        let fd = swiftlib::io::open(path, io::O_RDONLY);
        if fd >= 0 {
            swiftlib::io::close(fd as u64);
            true
        } else {
            false
        }
    }

    fn busybox_fallback_in_path(&self) -> Option<String> {
        let path_val = self.get_env("PATH").unwrap_or_default();
        for dir in path_val.split(':') {
            let dir = dir.trim();
            if dir.is_empty() {
                continue;
            }
            let candidate = format!("{}/busybox.elf", dir);
            if self.command_exists(&candidate) {
                return Some(candidate);
            }
        }
        None
    }

    pub fn new(fb_ptr: *mut u32, info: vga::FbInfo, font: Font) -> Self {
        let max_cols = info.width / FONT_WIDTH as u32;
        let max_rows = info.height / FONT_HEIGHT as u32;
        let mut env = Vec::new();
        env.push(("PATH".to_string(), "/Binaries".to_string()));
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
            ansi_seq: [0; ANSI_MAX_SEQ_LEN],
            ansi_seq_len: 0,
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
    fn find_in_path(&self, cmd: &str) -> Option<String> {
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
                return Some(candidate);
            }
        }
        None
    }

    fn put_pixel(&self, x: u32, y: u32, color: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = (y * self.stride + x) as usize;
        unsafe { self.fb_ptr.add(offset).write_volatile(color); }
    }

    fn draw_char(&self, ch: u8, col: u32, row: u32) {
        let glyph = *self.font.glyph(ch);
        let x0 = col * FONT_WIDTH as u32;
        let y0 = row * FONT_HEIGHT as u32;
        // 1フォント行を一括コピーすることで MMIO 書き込み回数を 72→12 に削減
        let mut row_buf = [0u32; FONT_WIDTH];
        for (r, &bits) in glyph.iter().enumerate() {
            let y = y0 + r as u32;
            if y >= self.height { break; }
            if x0 + FONT_WIDTH as u32 > self.width { break; }
            for c in 0..FONT_WIDTH {
                let on = (bits >> (7 - c)) & 1 != 0;
                row_buf[c] = if on { self.fg } else { self.bg };
            }
            // row_buf → フレームバッファ（スタック→MMIO の bulk write）
            let offset = (y * self.stride + x0) as usize;
            unsafe {
                core::ptr::copy_nonoverlapping(
                    row_buf.as_ptr(),
                    self.fb_ptr.add(offset),
                    FONT_WIDTH,
                );
            }
        }
    }

    pub fn clear_screen(&mut self) {
        // bg = 0x00000000 なので write_bytes(0) で全ピクセルをゼロ埋め（高速）
        let total_bytes = (self.height * self.stride) as usize * 4;
        unsafe {
            core::ptr::write_bytes(self.fb_ptr as *mut u8, 0, total_bytes);
        }
        self.col = 0;
        self.row = 0;
    }

    fn scroll_up(&mut self) {
        let row_pixels = FONT_HEIGHT as u32 * self.stride;
        let total = self.height * self.stride;
        // 本体をコピー（MMIO read + write だが memmove 相当で効率的）
        unsafe {
            let src = self.fb_ptr.add(row_pixels as usize);
            core::ptr::copy(src, self.fb_ptr, (total - row_pixels) as usize);
        }
        // 最終行をゼロ埋め（write_bytes で高速クリア）
        let last_row_start = (self.height - FONT_HEIGHT as u32) * self.stride;
        let clear_bytes = (FONT_HEIGHT as u32 * self.stride) as usize * 4;
        unsafe {
            core::ptr::write_bytes(
                self.fb_ptr.add(last_row_start as usize) as *mut u8,
                0,
                clear_bytes,
            );
        }
        self.row = self.max_rows - 1;
    }

    /// 互換性のために残す（シャドウバッファ廃止により no-op）
    pub fn flush(&mut self) {}

    fn new_line(&mut self) {
        self.col = 0;
        self.row += 1;
        if self.row >= self.max_rows {
            self.scroll_up();
        }
    }

    pub fn write_byte(&mut self, byte: u8) {
        match byte {
            b'\n' => self.new_line(),
            b'\r' => { self.col = 0; }
            0x08 => { // Backspace
                if self.col > 0 {
                    self.col -= 1;
                    self.draw_char(b' ', self.col, self.row);
                }
            }
            _ => {
                if self.col >= self.max_cols {
                    self.new_line();
                }
                self.draw_char(byte, self.col, self.row);
                self.col += 1;
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
        if self.ansi_seq_len == 0 {
            self.apply_sgr_code(0);
            return;
        }

        let mut start = 0usize;
        for i in 0..=self.ansi_seq_len {
            if i == self.ansi_seq_len || self.ansi_seq[i] == b';' {
                let code = if i == start {
                    0
                } else {
                    match Self::parse_ascii_u16(&self.ansi_seq[start..i]) {
                        Some(v) => v,
                        None => {
                            start = i + 1;
                            continue;
                        }
                    }
                };
                self.apply_sgr_code(code);
                start = i + 1;
            }
        }
    }

    fn reset_ansi_parser(&mut self) {
        self.ansi_esc_pending = false;
        self.ansi_csi_mode = false;
        self.ansi_seq_len = 0;
    }

    fn write_output_byte(&mut self, byte: u8) {
        if self.ansi_esc_pending {
            self.ansi_esc_pending = false;
            if byte == b'[' {
                self.ansi_csi_mode = true;
                self.ansi_seq_len = 0;
            } else if byte == 0x1B {
                self.ansi_esc_pending = true;
            } else {
                self.write_byte(byte);
            }
            return;
        }

        if self.ansi_csi_mode {
            if byte == b'm' {
                self.apply_sgr_sequence();
                self.reset_ansi_parser();
                return;
            }

            if byte.is_ascii_digit() || byte == b';' {
                if self.ansi_seq_len < self.ansi_seq.len() {
                    self.ansi_seq[self.ansi_seq_len] = byte;
                    self.ansi_seq_len += 1;
                } else {
                    self.reset_ansi_parser();
                }
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

    /// 子プロセスのIPC出力を受け取りながら終了を待つ
    fn drain_child_output(&mut self, pid: u64) {
        let mut buf = [0u8; 512];
        loop {
            // メッセージが届くまでスリープして待機（ビジーウェイトしない）
            let (_, len) = ipc::ipc_recv_wait(&mut buf);
            if len > 0 && len as usize <= buf.len() {
                if let Ok(s) = core::str::from_utf8(&buf[..len as usize]) {
                    self.write_str(s);
                }
                // 続きのメッセージをノンブロッキングで掃き出す
                loop {
                    let (_, len2) = ipc::ipc_recv(&mut buf);
                    if len2 == 0 || len2 as usize > buf.len() {
                        break;
                    }
                    if let Ok(s) = core::str::from_utf8(&buf[..len2 as usize]) {
                        self.write_str(s);
                    }
                }
                // バッチ分まとめてフラッシュ
                self.flush();
            }
            // 子プロセスが終了していれば抜ける（exit 通知で起床した場合もここで検知）
            if task::wait_nonblocking(pid as i64).is_some() {
                break;
            }
        }
        // 終了後に残ったメッセージを念のため掃き出す
        loop {
            let (_, len) = ipc::ipc_recv(&mut buf);
            if len == 0 || len as usize > buf.len() {
                break;
            }
            if let Ok(s) = core::str::from_utf8(&buf[..len as usize]) {
                self.write_str(s);
            }
        }
        self.flush();
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
                self.write_str("Commands: help, clear, version, export, cd\n");
                self.write_str("Other commands are loaded from PATH (Binaries/*.elf)\n");
                self.write_str("BusyBox applets are available via aliases (e.g. 'ls' -> busybox ls)\n");
            }
            "clear" => {
                self.clear_screen();
            }
            "version" => {
                self.write_str("mochiOS shell v0.1\n");
            }
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
                // export VAR=VALUE
                if let Some(eq) = joined_args.find('=') {
                    let key = joined_args[..eq].trim();
                    let val = joined_args[eq + 1..].trim();
                    let key_owned = key.to_string();
                    let val_owned = val.to_string();
                    self.set_env(&key_owned, &val_owned);
                } else {
                    self.write_str("usage: export VAR=VALUE\n");
                }
            }
            _ => {
                // PATH からコマンドを探して実行
                let mut path = self.find_in_path(cmd_name).map(|s| s.to_string());
                if path.is_none() && cmd_name != "busybox" && cmd_name != "busybox.elf" {
                    path = self.busybox_fallback_in_path();
                }
                match path {
                    Some(bin_path) => {
                        let mut arg_parts: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                        // busybox は applet 名を argv[1] に要求するため補完する。
                        if (cmd_name == "busybox" || cmd_name == "busybox.elf")
                            && arg_parts.is_empty()
                        {
                            // no-op (usage 表示)
                        } else if cmd_name != "busybox"
                            && cmd_name != "busybox.elf"
                            && bin_path.ends_with("/busybox.elf")
                        {
                            arg_parts.insert(0, cmd_name);
                        }
                        let result = exec_via_fs_service(&bin_path, &arg_parts);
                        match result {
                            Ok(pid) => {
                                // 子プロセスの出力をIPCで受け取りながら終了を待つ
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
