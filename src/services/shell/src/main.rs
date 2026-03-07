mod char;
mod keyboard;

use char::{Font, Terminal};
use keyboard::Ps2Keyboard;
use swiftlib::{time, vga};

fn main() {
    let info = match vga::get_info() {
        Some(i) => i,
        None => return,
    };
    
    let fb_ptr = match vga::map_framebuffer() {
        Some(p) => p,
        None => return,
    };

    let font = match Font::load() {
        Some(f) => f,
        None => return,
    };
    
    let mut term = Terminal::new(fb_ptr, info, font);
    let mut kbd = Ps2Keyboard::new();

    term.clear_screen();
    term.fg = 0x00FF_FF00; // 黄色
    term.write_str("mochiOS Shell\n");
    term.write_str("Type 'help' for commands.\n\n");
    term.fg = 0x00FF_FFFF;
    term.prompt();

    loop {
        time::sleep_ms(10);

        while let Some(ch) = kbd.read() {
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
