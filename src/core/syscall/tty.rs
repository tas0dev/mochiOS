//! シンプルなTTYレイヤ
//!
//! 1台のコンソールを想定し、termios/winsize と stdin の最小ラインディシプリンを提供する。

use crate::interrupt::spinlock::SpinLock;
use crate::syscall::{copy_to_user, EFAULT, EINVAL, SUCCESS};

const TERMIOS_SIZE: u64 = 36;
const WIN_SIZE: u64 = 8;

const LFLAG_ISIG: u32 = 0x0001;
const LFLAG_ICANON: u32 = 0x0002;
const LFLAG_ECHO: u32 = 0x0008;

const SC_LSHIFT: u8 = 0x2A;
const SC_RSHIFT: u8 = 0x36;
const SC_CAPSLOCK: u8 = 0x3A;
const SC_BACKSPACE: u8 = 0x0E;
const SC_ENTER: u8 = 0x1C;
const SC_TAB: u8 = 0x0F;
const SC_ESC: u8 = 0x01;
const SC_RELEASE: u8 = 0x80;
const SC_E0: u8 = 0xE0;

#[rustfmt::skip]
const MAP_NORMAL: [u8; 128] = [
    0,    0x1B, b'1', b'2', b'3', b'4', b'5', b'6',
    b'7', b'8', b'9', b'0', b'-', b'=', 0x08, b'\t',
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i',
    b'o', b'p', b'[', b']', b'\n', 0,   b'a', b's',
    b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';',
    b'\'',b'`', 0,   b'\\',b'z', b'x', b'c', b'v',
    b'b', b'n', b'm', b',', b'.', b'/', 0,   b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    b'7',
    b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1',
    b'2', b'3', b'0', b'.', 0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
];

#[rustfmt::skip]
const MAP_SHIFT: [u8; 128] = [
    0,    0x1B, b'!', b'@', b'#', b'$', b'%', b'^',
    b'&', b'*', b'(', b')', b'_', b'+', 0x08, b'\t',
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I',
    b'O', b'P', b'{', b'}', b'\n', 0,   b'A', b'S',
    b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':',
    b'"', b'~', 0,   b'|', b'Z', b'X', b'C', b'V',
    b'B', b'N', b'M', b'<', b'>', b'?', 0,   b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    b'7',
    b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1',
    b'2', b'3', b'0', b'.', 0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
];

#[derive(Clone, Copy)]
struct TtyState {
    iflag: u32,
    oflag: u32,
    cflag: u32,
    lflag: u32,
    line: u8,
    cc: [u8; 19],
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
    shift: bool,
    caps: bool,
    e0_prefix: bool,
}

impl TtyState {
    const fn new() -> Self {
        let mut cc = [0u8; 19];
        cc[6] = 1; // VMIN
        cc[5] = 0; // VTIME
        Self {
            iflag: 0,
            oflag: 0,
            cflag: 0x30 | 0x80 | 0x800,
            lflag: LFLAG_ISIG | LFLAG_ICANON | LFLAG_ECHO,
            line: 0,
            cc,
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
            shift: false,
            caps: false,
            e0_prefix: false,
        }
    }
}

static TTY_STATE: SpinLock<TtyState> = SpinLock::new(TtyState::new());
static INPUT_QUEUE: crate::util::fifo::Fifo<u8, 1024> = crate::util::fifo::Fifo::new();

fn push_bytes(bytes: &[u8]) {
    for &b in bytes {
        let _ = INPUT_QUEUE.push(b);
    }
}

fn decode_scancode_into_queue(sc: u8) {
    let mut state = TTY_STATE.lock();
    if state.e0_prefix {
        state.e0_prefix = false;
        if (sc & SC_RELEASE) != 0 {
            return;
        }
        match sc {
            0x48 => push_bytes(b"\x1b[A"), // Up
            0x50 => push_bytes(b"\x1b[B"), // Down
            0x4D => push_bytes(b"\x1b[C"), // Right
            0x4B => push_bytes(b"\x1b[D"), // Left
            0x47 => push_bytes(b"\x1b[H"), // Home
            0x4F => push_bytes(b"\x1b[F"), // End
            0x49 => push_bytes(b"\x1b[5~"),
            0x51 => push_bytes(b"\x1b[6~"),
            0x52 => push_bytes(b"\x1b[2~"),
            0x53 => push_bytes(b"\x1b[3~"),
            _ => {}
        }
        return;
    }

    if sc == SC_E0 {
        state.e0_prefix = true;
        return;
    }

    if (sc & SC_RELEASE) != 0 {
        let make = sc & !SC_RELEASE;
        if make == SC_LSHIFT || make == SC_RSHIFT {
            state.shift = false;
        }
        return;
    }

    match sc {
        SC_LSHIFT | SC_RSHIFT => {
            state.shift = true;
            return;
        }
        SC_CAPSLOCK => {
            state.caps = !state.caps;
            return;
        }
        SC_BACKSPACE => {
            let _ = INPUT_QUEUE.push(0x7F);
            return;
        }
        SC_ENTER => {
            let _ = INPUT_QUEUE.push(b'\n');
            return;
        }
        SC_TAB => {
            let _ = INPUT_QUEUE.push(b'\t');
            return;
        }
        SC_ESC => {
            let _ = INPUT_QUEUE.push(0x1B);
            return;
        }
        _ => {}
    }

    let idx = sc as usize;
    if idx >= MAP_NORMAL.len() {
        return;
    }
    let normal = MAP_NORMAL[idx];
    if normal == 0 {
        return;
    }
    let use_shift = state.shift ^ (state.caps && normal.is_ascii_alphabetic());
    let ch = if use_shift { MAP_SHIFT[idx] } else { normal };
    if ch != 0 {
        let _ = INPUT_QUEUE.push(ch);
    }
}

fn feed_from_scancode_queue_nonblocking() {
    while let Some(sc) = crate::util::ps2kbd::pop_scancode() {
        decode_scancode_into_queue(sc);
    }
}

fn next_input_byte_blocking() -> u8 {
    loop {
        if let Some(b) = INPUT_QUEUE.pop() {
            return b;
        }
        feed_from_scancode_queue_nonblocking();
        if let Some(b) = INPUT_QUEUE.pop() {
            return b;
        }
        let sc = crate::syscall::keyboard::read_char_blocking();
        decode_scancode_into_queue(sc);
    }
}

pub fn tcgets(arg: u64) -> u64 {
    if arg == 0 || !crate::syscall::validate_user_ptr(arg, TERMIOS_SIZE) {
        return EINVAL;
    }
    let state = *TTY_STATE.lock();
    crate::syscall::with_user_memory_access(|| unsafe {
        let buf = core::slice::from_raw_parts_mut(arg as *mut u8, TERMIOS_SIZE as usize);
        buf.fill(0);
        buf[0..4].copy_from_slice(&state.iflag.to_ne_bytes());
        buf[4..8].copy_from_slice(&state.oflag.to_ne_bytes());
        buf[8..12].copy_from_slice(&state.cflag.to_ne_bytes());
        buf[12..16].copy_from_slice(&state.lflag.to_ne_bytes());
        buf[16] = state.line;
        buf[17..36].copy_from_slice(&state.cc);
    });
    SUCCESS
}

pub fn tcsets(arg: u64) -> u64 {
    if arg == 0 || !crate::syscall::validate_user_ptr(arg, TERMIOS_SIZE) {
        return EINVAL;
    }
    let (iflag, oflag, cflag, lflag, line, cc) = crate::syscall::with_user_memory_access(|| unsafe {
        let p = arg as *const u8;
        let iflag = u32::from_ne_bytes([*p.add(0), *p.add(1), *p.add(2), *p.add(3)]);
        let oflag = u32::from_ne_bytes([*p.add(4), *p.add(5), *p.add(6), *p.add(7)]);
        let cflag = u32::from_ne_bytes([*p.add(8), *p.add(9), *p.add(10), *p.add(11)]);
        let lflag = u32::from_ne_bytes([*p.add(12), *p.add(13), *p.add(14), *p.add(15)]);
        let line = *p.add(16);
        let mut cc = [0u8; 19];
        for (i, v) in cc.iter_mut().enumerate() {
            *v = *p.add(17 + i);
        }
        (iflag, oflag, cflag, lflag, line, cc)
    });
    let mut state = TTY_STATE.lock();
    state.iflag = iflag;
    state.oflag = oflag;
    state.cflag = cflag;
    state.lflag = lflag;
    state.line = line;
    state.cc = cc;
    SUCCESS
}

pub fn get_winsize(arg: u64) -> u64 {
    if arg == 0 || !crate::syscall::validate_user_ptr(arg, WIN_SIZE) {
        return EINVAL;
    }
    let state = *TTY_STATE.lock();
    crate::syscall::with_user_memory_access(|| unsafe {
        let buf = core::slice::from_raw_parts_mut(arg as *mut u8, WIN_SIZE as usize);
        buf[0..2].copy_from_slice(&state.ws_row.to_ne_bytes());
        buf[2..4].copy_from_slice(&state.ws_col.to_ne_bytes());
        buf[4..6].copy_from_slice(&state.ws_xpixel.to_ne_bytes());
        buf[6..8].copy_from_slice(&state.ws_ypixel.to_ne_bytes());
    });
    SUCCESS
}

pub fn set_winsize(arg: u64) -> u64 {
    if arg == 0 || !crate::syscall::validate_user_ptr(arg, WIN_SIZE) {
        return EINVAL;
    }
    let (row, col, xpixel, ypixel) = crate::syscall::with_user_memory_access(|| unsafe {
        let p = arg as *const u8;
        let row = u16::from_ne_bytes([*p.add(0), *p.add(1)]);
        let col = u16::from_ne_bytes([*p.add(2), *p.add(3)]);
        let xpixel = u16::from_ne_bytes([*p.add(4), *p.add(5)]);
        let ypixel = u16::from_ne_bytes([*p.add(6), *p.add(7)]);
        (row, col, xpixel, ypixel)
    });
    let mut state = TTY_STATE.lock();
    if row != 0 {
        state.ws_row = row;
    }
    if col != 0 {
        state.ws_col = col;
    }
    state.ws_xpixel = xpixel;
    state.ws_ypixel = ypixel;
    SUCCESS
}

pub fn read_stdin(buf_ptr: u64, len: u64) -> u64 {
    if buf_ptr == 0 || len == 0 {
        return EFAULT;
    }
    if !crate::syscall::validate_user_ptr(buf_ptr, len) {
        return EFAULT;
    }

    let canonical = (TTY_STATE.lock().lflag & LFLAG_ICANON) != 0;
    let mut out = alloc::vec::Vec::with_capacity(len as usize);
    let first = next_input_byte_blocking();
    out.push(first);

    if canonical {
        while (out.len() as u64) < len {
            if out.last().copied() == Some(b'\n') {
                break;
            }
            out.push(next_input_byte_blocking());
        }
    } else {
        feed_from_scancode_queue_nonblocking();
        while (out.len() as u64) < len {
            match INPUT_QUEUE.pop() {
                Some(b) => out.push(b),
                None => break,
            }
        }
    }

    if let Err(errno) = copy_to_user(buf_ptr, &out) {
        return errno;
    }
    out.len() as u64
}
