//! シンプルなTTYレイヤ
//!
//! 1台のコンソールを想定し、termios/winsize と stdin の最小ラインディシプリンを提供する。

use crate::interrupt::spinlock::SpinLock;
use crate::syscall::{copy_to_user, EFAULT, EINVAL, SUCCESS};
use alloc::vec::Vec;

const TERMIOS_SIZE: u64 = 36;
const TERMIO_SIZE: u64 = 18;
const WIN_SIZE: u64 = 8;

const IFLAG_ICRNL: u32 = 0x0100;
const LFLAG_ISIG: u32 = 0x0001;
const LFLAG_ICANON: u32 = 0x0002;
const LFLAG_ECHO: u32 = 0x0008;
const CC_VTIME: usize = 5;
const CC_VMIN: usize = 6;

const SC_LSHIFT: u8 = 0x2A;
const SC_RSHIFT: u8 = 0x36;
const SC_CAPSLOCK: u8 = 0x3A;
const SC_BACKSPACE: u8 = 0x0E;
const SC_ENTER: u8 = 0x1C;
const SC_TAB: u8 = 0x0F;
const SC_ESC: u8 = 0x01;
const SC_RELEASE: u8 = 0x80;
const SC_E0: u8 = 0xE0;
const OUT_CSI_MAX_SEQ: usize = 32;

#[rustfmt::skip]
const MAP_NORMAL: [u8; 128] = [
    0,    0x1B, b'1', b'2', b'3', b'4', b'5', b'6',
    b'7', b'8', b'9', b'0', b'-', b'=', 0x08, b'\t',
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i',
    b'o', b'p', b'[', b']', b'\n', 0,   b'a', b's',
    b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';',
    b':', b'`', 0,   b'\\',b'z', b'x', b'c', b'v',
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
    b'*', b'~', 0,   b'|', b'Z', b'X', b'C', b'V',
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
    out_esc_pending: bool,
    out_csi_mode: bool,
    out_csi_seq: [u8; OUT_CSI_MAX_SEQ],
    out_csi_len: usize,
    cursor_row: u16, // 1-origin
    cursor_col: u16, // 1-origin
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
            // 対話アプリ(vim等)を優先し、既定は非canonical/非echoで開始する。
            // shell.service は独自の行編集を行うため、この既定値でも影響しない。
            lflag: LFLAG_ISIG,
            line: 0,
            cc,
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
            shift: false,
            caps: false,
            e0_prefix: false,
            out_esc_pending: false,
            out_csi_mode: false,
            out_csi_seq: [0; OUT_CSI_MAX_SEQ],
            out_csi_len: 0,
            cursor_row: 1,
            cursor_col: 1,
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

fn push_ascii_u16(out: &mut Vec<u8>, mut n: u16) {
    if n == 0 {
        out.push(b'0');
        return;
    }
    let mut buf = [0u8; 5];
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    out.extend_from_slice(&buf[i..]);
}

fn clamp_cursor(state: &mut TtyState) {
    let rows = state.ws_row.max(1);
    let cols = state.ws_col.max(1);
    if state.cursor_row == 0 {
        state.cursor_row = 1;
    } else if state.cursor_row > rows {
        state.cursor_row = rows;
    }
    if state.cursor_col == 0 {
        state.cursor_col = 1;
    } else if state.cursor_col > cols {
        state.cursor_col = cols;
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

fn csi_params(seq: &[u8]) -> (Option<u8>, [u16; 8], usize) {
    let mut params = [0u16; 8];
    let mut count = 0usize;
    let mut start = 0usize;
    let private = match seq.first().copied() {
        Some(b'?') | Some(b'>') | Some(b'!') => {
            start = 1;
            Some(seq[0])
        }
        _ => None,
    };
    let mut i = start;
    let mut part_start = start;
    while i <= seq.len() && count < params.len() {
        if i == seq.len() || seq[i] == b';' {
            if i == part_start {
                params[count] = 0;
                count += 1;
            } else if let Some(v) = parse_ascii_u16(&seq[part_start..i]) {
                params[count] = v;
                count += 1;
            }
            part_start = i + 1;
        }
        i += 1;
    }
    (private, params, count)
}

fn csi_param_or(params: &[u16; 8], count: usize, idx: usize, default: u16) -> u16 {
    let v = if idx < count { params[idx] } else { default };
    if v == 0 { default } else { v }
}

fn parse_decrqm_mode(seq: &[u8]) -> Option<u16> {
    // `CSI ? Pm $ p` の Pm を抜き出す
    if seq.len() < 3 || seq[0] != b'?' || *seq.last()? != b'$' {
        return None;
    }
    parse_ascii_u16(&seq[1..seq.len() - 1])
}

fn handle_output_csi(state: &mut TtyState, final_byte: u8, replies: &mut Vec<u8>) {
    let rows = state.ws_row.max(1);
    let cols = state.ws_col.max(1);
    let seq = &state.out_csi_seq[..state.out_csi_len];
    let (private, params, count) = csi_params(seq);
    match final_byte {
        b'A' => {
            let n = csi_param_or(&params, count, 0, 1);
            state.cursor_row = state.cursor_row.saturating_sub(n);
            if state.cursor_row == 0 {
                state.cursor_row = 1;
            }
        }
        b'B' => {
            let n = csi_param_or(&params, count, 0, 1);
            state.cursor_row = core::cmp::min(state.cursor_row.saturating_add(n), rows);
        }
        b'C' => {
            let n = csi_param_or(&params, count, 0, 1);
            state.cursor_col = core::cmp::min(state.cursor_col.saturating_add(n), cols);
        }
        b'D' => {
            let n = csi_param_or(&params, count, 0, 1);
            state.cursor_col = state.cursor_col.saturating_sub(n);
            if state.cursor_col == 0 {
                state.cursor_col = 1;
            }
        }
        b'H' | b'f' => {
            state.cursor_row = csi_param_or(&params, count, 0, 1);
            state.cursor_col = csi_param_or(&params, count, 1, 1);
            clamp_cursor(state);
        }
        b'G' => {
            state.cursor_col = csi_param_or(&params, count, 0, 1);
            clamp_cursor(state);
        }
        b'd' => {
            state.cursor_row = csi_param_or(&params, count, 0, 1);
            clamp_cursor(state);
        }
        b'J' => {
            let mode = if count > 0 { params[0] } else { 0 };
            if mode == 2 {
                state.cursor_row = 1;
                state.cursor_col = 1;
            }
        }
        b'n' => {
            // DSR: Device Status/CPR
            let p0 = csi_param_or(&params, count, 0, 0);
            if p0 == 5 {
                // "OK" status report
                if private == Some(b'?') {
                    replies.extend_from_slice(b"\x1b[?0n");
                } else {
                    replies.extend_from_slice(b"\x1b[0n");
                }
            } else if p0 == 6 {
                // Cursor Position Report
                replies.extend_from_slice(b"\x1b[");
                if private == Some(b'?') {
                    replies.push(b'?');
                }
                push_ascii_u16(replies, state.cursor_row.max(1));
                replies.push(b';');
                push_ascii_u16(replies, state.cursor_col.max(1));
                replies.push(b'R');
            }
        }
        b'c' => {
            // DA / Secondary DA (xterm 互換の代表値を返す)
            if private == Some(b'>') {
                replies.extend_from_slice(b"\x1b[>0;136;0c");
            } else {
                replies.extend_from_slice(b"\x1b[?1;2c");
            }
        }
        b'p' => {
            // DECRQM: `CSI ? Pm $ p` への応答
            if private == Some(b'?') {
                if let Some(mode) = parse_decrqm_mode(seq) {
                    replies.extend_from_slice(b"\x1b[?");
                    push_ascii_u16(replies, mode);
                    // 0 = unsupported / unknown
                    replies.extend_from_slice(b";0$y");
                }
            }
        }
        _ => {}
    }
}

pub fn process_output(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let mut replies: Vec<u8> = Vec::new();
    {
        let mut state = TTY_STATE.lock();
        let rows = state.ws_row.max(1);
        let cols = state.ws_col.max(1);
        for &b in bytes {
            if state.out_csi_mode {
                if (0x20..=0x3F).contains(&b) {
                    if state.out_csi_len < state.out_csi_seq.len() {
                        let idx = state.out_csi_len;
                        state.out_csi_seq[idx] = b;
                        state.out_csi_len += 1;
                    } else {
                        state.out_csi_mode = false;
                        state.out_csi_len = 0;
                    }
                    continue;
                }
                if (0x40..=0x7E).contains(&b) {
                    handle_output_csi(&mut state, b, &mut replies);
                    state.out_csi_mode = false;
                    state.out_csi_len = 0;
                    continue;
                }
                state.out_csi_mode = false;
                state.out_csi_len = 0;
                continue;
            }

            if state.out_esc_pending {
                state.out_esc_pending = false;
                if b == b'[' {
                    state.out_csi_mode = true;
                    state.out_csi_len = 0;
                    continue;
                }
                continue;
            }

            match b {
                0x1B => state.out_esc_pending = true,
                b'\r' => state.cursor_col = 1,
                b'\n' => {
                    if state.cursor_row < rows {
                        state.cursor_row += 1;
                    }
                }
                0x08 => {
                    if state.cursor_col > 1 {
                        state.cursor_col -= 1;
                    }
                }
                b'\t' => {
                    let cur0 = state.cursor_col.saturating_sub(1);
                    let next = ((cur0 / 8) + 1) * 8 + 1;
                    state.cursor_col = core::cmp::min(next, cols);
                }
                0x20..=0x7E => {
                    if state.cursor_col >= cols {
                        state.cursor_col = 1;
                        if state.cursor_row < rows {
                            state.cursor_row += 1;
                        }
                    } else {
                        state.cursor_col += 1;
                    }
                }
                _ => {}
            }
        }
        clamp_cursor(&mut state);
    }
    if !replies.is_empty() {
        push_bytes(&replies);
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
            let _ = INPUT_QUEUE.push(b'\r');
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

pub fn has_pending_input() -> bool {
    feed_from_scancode_queue_nonblocking();
    !INPUT_QUEUE.is_empty()
}

pub fn pending_input_len() -> usize {
    feed_from_scancode_queue_nonblocking();
    INPUT_QUEUE.len()
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

fn next_input_byte_nonblocking() -> Option<u8> {
    if let Some(b) = INPUT_QUEUE.pop() {
        return Some(b);
    }
    feed_from_scancode_queue_nonblocking();
    INPUT_QUEUE.pop()
}

fn next_input_byte_timeout(timeout_ms: u64) -> Option<u8> {
    if let Some(b) = next_input_byte_nonblocking() {
        return Some(b);
    }
    if timeout_ms == 0 {
        return None;
    }
    let mut remain = timeout_ms;
    while remain > 0 {
        crate::syscall::process::sleep(1);
        if let Some(b) = next_input_byte_nonblocking() {
            return Some(b);
        }
        remain -= 1;
    }
    None
}

#[inline]
fn normalize_input_byte(b: u8, iflag: u32) -> u8 {
    if b == b'\r' && (iflag & IFLAG_ICRNL) != 0 {
        b'\n'
    } else {
        b
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
    let (iflag, oflag, cflag, lflag, line, cc) =
        crate::syscall::with_user_memory_access(|| unsafe {
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
    if (state.lflag & LFLAG_ICANON) == 0 {
        state.cc[CC_VMIN] = 1;
        state.cc[CC_VTIME] = 0;
    }
    SUCCESS
}

pub fn tcgeta(arg: u64) -> u64 {
    if arg == 0 || !crate::syscall::validate_user_ptr(arg, TERMIO_SIZE) {
        return EINVAL;
    }
    let state = *TTY_STATE.lock();
    crate::syscall::with_user_memory_access(|| unsafe {
        let buf = core::slice::from_raw_parts_mut(arg as *mut u8, TERMIO_SIZE as usize);
        buf.fill(0);
        let iflag = (state.iflag & 0xFFFF) as u16;
        let oflag = (state.oflag & 0xFFFF) as u16;
        let cflag = (state.cflag & 0xFFFF) as u16;
        let lflag = (state.lflag & 0xFFFF) as u16;
        buf[0..2].copy_from_slice(&iflag.to_ne_bytes());
        buf[2..4].copy_from_slice(&oflag.to_ne_bytes());
        buf[4..6].copy_from_slice(&cflag.to_ne_bytes());
        buf[6..8].copy_from_slice(&lflag.to_ne_bytes());
        buf[8] = state.line;
        buf[9..18].copy_from_slice(&state.cc[..9]);
    });
    SUCCESS
}

pub fn tcseta(arg: u64) -> u64 {
    if arg == 0 || !crate::syscall::validate_user_ptr(arg, TERMIO_SIZE) {
        return EINVAL;
    }
    let (iflag, oflag, cflag, lflag, line, cc9) =
        crate::syscall::with_user_memory_access(|| unsafe {
            let p = arg as *const u8;
            let iflag = u16::from_ne_bytes([*p.add(0), *p.add(1)]) as u32;
            let oflag = u16::from_ne_bytes([*p.add(2), *p.add(3)]) as u32;
            let cflag = u16::from_ne_bytes([*p.add(4), *p.add(5)]) as u32;
            let lflag = u16::from_ne_bytes([*p.add(6), *p.add(7)]) as u32;
            let line = *p.add(8);
            let mut cc9 = [0u8; 9];
            for (i, v) in cc9.iter_mut().enumerate() {
                *v = *p.add(9 + i);
            }
            (iflag, oflag, cflag, lflag, line, cc9)
        });
    let mut state = TTY_STATE.lock();
    state.iflag = iflag;
    state.oflag = oflag;
    state.cflag = cflag;
    state.lflag = lflag;
    state.line = line;
    state.cc[..9].copy_from_slice(&cc9);
    if (state.lflag & LFLAG_ICANON) == 0 {
        state.cc[CC_VMIN] = 1;
        state.cc[CC_VTIME] = 0;
    }
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
    clamp_cursor(&mut state);
    SUCCESS
}

pub fn read_stdin(buf_ptr: u64, len: u64) -> u64 {
    if buf_ptr == 0 || len == 0 {
        return EFAULT;
    }
    if !crate::syscall::validate_user_ptr(buf_ptr, len) {
        return EFAULT;
    }

    let state = *TTY_STATE.lock();
    let canonical = (state.lflag & LFLAG_ICANON) != 0;
    let iflag = state.iflag;
    let mut out = alloc::vec::Vec::with_capacity(len as usize);
    if canonical {
        let first = normalize_input_byte(next_input_byte_blocking(), iflag);
        out.push(first);
        while (out.len() as u64) < len {
            if out.last().copied() == Some(b'\n') {
                break;
            }
            let b = normalize_input_byte(next_input_byte_blocking(), iflag);
            out.push(b);
        }
    } else {
        // 対話アプリ優先: noncanonical は raw 1バイト即返却。
        let b = match next_input_byte_nonblocking() {
            Some(v) => v,
            None => next_input_byte_blocking(),
        };
        out.push(normalize_input_byte(b, iflag));
    }

    if let Err(errno) = copy_to_user(buf_ptr, &out) {
        return errno;
    }
    out.len() as u64
}
