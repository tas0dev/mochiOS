use swiftlib::keyboard;
use swiftlib::time;
use swiftlib::vga;

const FONT_WIDTH: usize = 6;
const FONT_HEIGHT: usize = 12;
const ASCII_START: usize = 32;
const ASCII_END: usize = 127;
const GLYPH_COUNT: usize = ASCII_END - ASCII_START;

// BDF フォントファイルをコンパイル時に埋め込む
static BDF_DATA: &[u8] = include_bytes!("../../../resources/fonts/ter-u12b.bdf");

/// ASCII 文字ごとの 12 行ビットマップ (インデックス = codepoint - 32)
struct Font {
    glyphs: [[u8; FONT_HEIGHT]; GLYPH_COUNT],
}

impl Font {
    fn new() -> Self {
        let mut font = Font { glyphs: [[0u8; FONT_HEIGHT]; GLYPH_COUNT] };
        parse_bdf(BDF_DATA, &mut font.glyphs);
        font
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
struct Terminal {
    fb_ptr: *mut u32,
    width: u32,
    height: u32,
    stride: u32,
    col: u32,
    row: u32,
    max_cols: u32,
    max_rows: u32,
    font: Font,
    fg: u32,
    bg: u32,
    input_buf: [u8; 256],
    input_len: usize,
}

impl Terminal {
    fn new(fb_ptr: *mut u32, info: vga::FbInfo, font: Font) -> Self {
        let max_cols = info.width / FONT_WIDTH as u32;
        let max_rows = info.height / FONT_HEIGHT as u32;
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
        }
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

    fn clear_screen(&mut self) {
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

    fn write_byte(&mut self, byte: u8) {
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

    fn write_str(&mut self, s: &str) {
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

    fn prompt(&mut self) {
        self.fg = 0x00FF_88FF; // 紫
        self.write_str("mochi> ");
        self.fg = 0x00FF_FFFF; // シアン
    }

    fn handle_line(&mut self) {
        // バッファから文字列をコピーして借用を解放
        let mut tmp = [0u8; 256];
        let len = self.input_len;
        tmp[..len].copy_from_slice(&self.input_buf[..len]);
        let cmd_str: &str = core::str::from_utf8(&tmp[..len]).unwrap_or("").trim();

        // コマンド内容をコピー（trim後のスライスはtmpを参照するため、ここで確定させる）
        let mut cmd_buf = [0u8; 256];
        let cmd_bytes = cmd_str.as_bytes();
        cmd_buf[..cmd_bytes.len()].copy_from_slice(cmd_bytes);
        let cmd_len = cmd_bytes.len();

        self.write_byte(b'\n');
        self.input_len = 0;

        let cmd = core::str::from_utf8(&cmd_buf[..cmd_len]).unwrap_or("");
        match cmd {
            "" => {}
            "help" => {
                self.write_str("Commands: help, clear, version\n");
            }
            "clear" => {
                self.clear_screen();
            }
            "version" => {
                self.write_str("mochiOS shell v0.1\n");
            }
            _ => {
                self.write_str("Unknown command: ");
                for &b in &cmd_buf[..cmd_len] {
                    self.write_byte(b);
                }
                self.write_byte(b'\n');
            }
        }
    }
}

fn main() {
    let info = match vga::get_info() {
        Some(i) => i,
        None => return,
    };
    let fb_ptr = match vga::map_framebuffer() {
        Some(p) => p,
        None => return,
    };

    let font = Font::new();
    let mut term = Terminal::new(fb_ptr, info, font);

    term.clear_screen();
    term.fg = 0x00FF_FF00; // 黄色
    term.write_str("mochiOS Shell\n");
    term.write_str("Type 'help' for commands.\n\n");
    term.fg = 0x00FF_FFFF;
    term.prompt();

    loop {
        time::sleep_ms(10);

        while let Some(ch) = keyboard::read_char() {
            match ch {
                b'\n' | b'\r' => {
                    term.handle_line();
                    term.prompt();
                }
                0x08 | 0x7F => { // Backspace / Delete
                    if term.input_len > 0 {
                        term.input_len -= 1;
                        term.write_byte(0x08);
                    }
                }
                0x20..=0x7E => {
                    if term.input_len < term.input_buf.len() - 1 {
                        term.input_buf[term.input_len] = ch;
                        term.input_len += 1;
                        term.write_byte(ch);
                    }
                }
                _ => {}
            }
        }
    }
}
