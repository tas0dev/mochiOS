use swiftlib::{process, vga};


const FONT_WIDTH: usize = 6;
const FONT_HEIGHT: usize = 12;
const ASCII_START: usize = 32;
const ASCII_END: usize = 127;
const GLYPH_COUNT: usize = ASCII_END - ASCII_START;

/// ASCII 文字ごとの 12 行ビットマップ (インデックス = codepoint - 32)
pub struct Font {
    glyphs: [[u8; FONT_HEIGHT]; GLYPH_COUNT],
}

impl Font {
    /// `System/fonts/ter-u12b.bdf` を読み込んで解析する
    pub fn load() -> Option<Self> {
        let data = std::fs::read("System/fonts/ter-u12b.bdf").ok()?;
        let mut font = Font { glyphs: [[0u8; FONT_HEIGHT]; GLYPH_COUNT] };
        parse_bdf(&data, &mut font.glyphs);
        Some(font)
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

    while let Some(line) = lines.next() {
        let line = line.trim();
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
}

#[allow(unused)]
impl Terminal {
    pub fn new(fb_ptr: *mut u32, info: vga::FbInfo, font: Font) -> Self {
        let max_cols = info.width / FONT_WIDTH as u32;
        let max_rows = info.height / FONT_HEIGHT as u32;
        let mut env = Vec::new();
        env.push(("PATH".to_string(), "Binaries".to_string()));
        Terminal {
            fb_ptr,
            width: info.width,
            height: info.height,
            stride: info.stride,
            col: 0,
            row: 0,
            max_cols,
            max_rows,
            font,
            fg: 0x00FF_FFFF, // シアン
            bg: 0x0000_0000, // 黒
            input_buf: [0u8; 256],
            input_len: 0,
            env,
        }
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

    /// PATH の各ディレクトリで `{cmd}.elf` を探す
    fn find_in_path(&self, cmd: &str) -> Option<String> {
        let path_val = self.get_env("PATH").unwrap_or_default();
        for dir in path_val.split(':') {
            let candidate = format!("{}/{}.elf", dir, cmd);
            if std::fs::metadata(&candidate).is_ok() {
                return Some(candidate);
            }
        }
        None
    }

    fn put_pixel(&self, x: u32, y: u32, color: u32) {
        if x >= self.width || y >= self.height {
            return;
        }
        let offset = y * self.stride + x;
        unsafe {
            self.fb_ptr.add(offset as usize).write_volatile(color);
        }
    }

    fn draw_char(&self, ch: u8, col: u32, row: u32) {
        let glyph = self.font.glyph(ch);
        let x0 = col * FONT_WIDTH as u32;
        let y0 = row * FONT_HEIGHT as u32;
        for (r, &bits) in glyph.iter().enumerate() {
            for c in 0..FONT_WIDTH {
                // BDF: bit 7 = 左端, 6ピクセル幅なので bit (7-c) を使う
                let on = (bits >> (7 - c)) & 1 != 0;
                self.put_pixel(x0 + c as u32, y0 + r as u32, if on { self.fg } else { self.bg });
            }
        }
    }

    pub fn clear_screen(&mut self) {
        let total = self.height * self.stride;
        for i in 0..total {
            unsafe { self.fb_ptr.add(i as usize).write_volatile(self.bg); }
        }
        self.col = 0;
        self.row = 0;
    }

    fn scroll_up(&mut self) {
        let row_pixels = FONT_HEIGHT as u32 * self.stride;
        let total = self.height * self.stride;
        // 1行分上にコピー
        unsafe {
            let src = self.fb_ptr.add(row_pixels as usize);
            core::ptr::copy(src, self.fb_ptr, (total - row_pixels) as usize);
        }
        // 最終行をクリア
        let last_row_start = (self.height - FONT_HEIGHT as u32) * self.stride;
        for i in 0..(FONT_HEIGHT as u32 * self.stride) {
            unsafe { self.fb_ptr.add((last_row_start + i) as usize).write_volatile(self.bg); }
        }
        self.row = self.max_rows - 1;
    }

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

    pub fn write_str(&mut self, s: &str) {
        for b in s.bytes() {
            self.write_byte(b);
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
        self.fg = 0x00FF_88FF; // 紫
        self.write_str("mochi> ");
        self.fg = 0x00FF_FFFF; // シアン
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

        // コマンド名と引数を分割
        let mut parts = cmd.splitn(2, ' ');
        let cmd_name = parts.next().unwrap_or("");
        let _args = parts.next().unwrap_or("");

        match cmd_name {
            "help" => {
                self.write_str("Commands: help, clear, version, export\n");
                self.write_str("Other commands are loaded from PATH (Binaries/*.elf)\n");
            }
            "clear" => {
                self.clear_screen();
            }
            "version" => {
                self.write_str("mochiOS shell v0.1\n");
            }
            "export" => {
                // export VAR=VALUE
                if let Some(eq) = _args.find('=') {
                    let key = _args[..eq].trim();
                    let val = _args[eq + 1..].trim();
                    let key_owned = key.to_string();
                    let val_owned = val.to_string();
                    self.set_env(&key_owned, &val_owned);
                } else {
                    self.write_str("usage: export VAR=VALUE\n");
                }
            }
            _ => {
                // PATH からコマンドを探して実行
                let path = self.find_in_path(cmd_name).map(|s| s.to_string());
                match path {
                    Some(bin_path) => {
                        match process::exec(&bin_path) {
                            Ok(_pid) => {}
                            Err(()) => {
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
