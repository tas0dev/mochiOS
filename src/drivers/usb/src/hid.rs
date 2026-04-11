use swiftlib::input;

use crate::define::SC_RELEASE;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HidReportKind {
    Keyboard,
    Mouse,
    Unknown,
}

impl Default for HidReportKind {
    fn default() -> Self {
        Self::Unknown
    }
}

#[derive(Default)]
pub struct HidParserState {
    prev_keys: [u8; 6],
    prev_modifiers: u8,
    warned_kbd_inject: bool,
    warned_mouse_inject: bool,
    mouse_entries: [MouseDecodeEntry; 8],
}

#[derive(Clone, Copy)]
struct MouseDecodeEntry {
    used: bool,
    slot: u8,
    ep: u8,
    prev_buttons: u8,
}

impl MouseDecodeEntry {
    const fn new() -> Self {
        Self {
            used: false,
            slot: 0,
            ep: 0,
            prev_buttons: 0,
        }
    }
}

impl Default for MouseDecodeEntry {
    fn default() -> Self {
        Self::new()
    }
}

fn mouse_entry_mut(state: &mut HidParserState, slot: u8, ep: u8) -> &mut MouseDecodeEntry {
    if let Some(idx) = state
        .mouse_entries
        .iter()
        .position(|e| e.used && e.slot == slot && e.ep == ep)
    {
        return &mut state.mouse_entries[idx];
    }
    if let Some(idx) = state.mouse_entries.iter().position(|e| !e.used) {
        state.mouse_entries[idx] = MouseDecodeEntry {
            used: true,
            slot,
            ep,
            prev_buttons: 0,
        };
        return &mut state.mouse_entries[idx];
    }
    // エントリ枯渇時は先頭を上書き
    state.mouse_entries[0] = MouseDecodeEntry {
        used: true,
        slot,
        ep,
        prev_buttons: 0,
    };
    &mut state.mouse_entries[0]
}

#[inline]
fn map_hid_usage_to_set1_scancode(usage: u8) -> Option<u8> {
    match usage {
        0x04 => Some(0x1E), // a
        0x05 => Some(0x30), // b
        0x06 => Some(0x2E), // c
        0x07 => Some(0x20), // d
        0x08 => Some(0x12), // e
        0x09 => Some(0x21), // f
        0x0A => Some(0x22), // g
        0x0B => Some(0x23), // h
        0x0C => Some(0x17), // i
        0x0D => Some(0x24), // j
        0x0E => Some(0x25), // k
        0x0F => Some(0x26), // l
        0x10 => Some(0x32), // m
        0x11 => Some(0x31), // n
        0x12 => Some(0x18), // o
        0x13 => Some(0x19), // p
        0x14 => Some(0x10), // q
        0x15 => Some(0x13), // r
        0x16 => Some(0x1F), // s
        0x17 => Some(0x14), // t
        0x18 => Some(0x16), // u
        0x19 => Some(0x2F), // v
        0x1A => Some(0x11), // w
        0x1B => Some(0x2D), // x
        0x1C => Some(0x15), // y
        0x1D => Some(0x2C), // z
        0x1E => Some(0x02), // 1
        0x1F => Some(0x03), // 2
        0x20 => Some(0x04), // 3
        0x21 => Some(0x05), // 4
        0x22 => Some(0x06), // 5
        0x23 => Some(0x07), // 6
        0x24 => Some(0x08), // 7
        0x25 => Some(0x09), // 8
        0x26 => Some(0x0A), // 9
        0x27 => Some(0x0B), // 0
        0x28 => Some(0x1C), // Enter
        0x29 => Some(0x01), // Esc
        0x2A => Some(0x0E), // Backspace
        0x2B => Some(0x0F), // Tab
        0x2C => Some(0x39), // Space
        0x2D => Some(0x0C), // -
        0x2E => Some(0x0D), // =
        0x2F => Some(0x1A), // [
        0x30 => Some(0x1B), // ]
        0x31 => Some(0x2B), // \
        0x33 => Some(0x27), // ;
        0x34 => Some(0x28), // '
        0x35 => Some(0x29), // `
        0x36 => Some(0x33), // ,
        0x37 => Some(0x34), // .
        0x38 => Some(0x35), // /
        _ => None,
    }
}

#[inline]
fn inject_scancode(scancode: u8, state: &mut HidParserState) {
    if let Err(_err) = input::inject_scancode(scancode) {
        if !state.warned_kbd_inject {
            state.warned_kbd_inject = true;
        }
    }
}

fn inject_modifier_transitions(new_mod: u8, state: &mut HidParserState) {
    const LCTRL: u8 = 0x1D;
    const LSHIFT: u8 = 0x2A;
    const LALT: u8 = 0x38;
    const LGUI: u8 = 0x5B;
    const RCTRL: u8 = 0x1D;
    const RSHIFT: u8 = 0x36;
    const RALT: u8 = 0x38;
    const RGUI: u8 = 0x5C;
    let old_mod = state.prev_modifiers;
    let pairs = [
        (0x01u8, false, LCTRL),
        (0x02u8, false, LSHIFT),
        (0x04u8, false, LALT),
        (0x08u8, true, LGUI),
        (0x10u8, true, RCTRL),
        (0x20u8, false, RSHIFT),
        (0x40u8, true, RALT),
        (0x80u8, true, RGUI),
    ];
    for (mask, e0, sc) in pairs {
        let was = (old_mod & mask) != 0;
        let now = (new_mod & mask) != 0;
        if !was && now {
            if e0 {
                inject_scancode(0xE0, state);
            }
            inject_scancode(sc, state);
        } else if was && !now {
            if e0 {
                inject_scancode(0xE0, state);
            }
            inject_scancode(sc | SC_RELEASE, state);
        }
    }
    state.prev_modifiers = new_mod;
}

fn parse_hid_keyboard_report(
    _slot: u8,
    _ep: u8,
    report: &[u8],
    state: &mut HidParserState,
    strict_usage_check: bool,
) -> bool {
    let mut chosen_offset = None;
    for offset in [0usize, 1usize] {
        if report.len() < offset + 8 {
            continue;
        }
        // Boot keyboard report は [modifiers, reserved, key0..key5]。
        // reserved が 0 でない場合はキーボードとして扱わない。
        if report[offset + 1] != 0 {
            continue;
        }
        chosen_offset = Some(offset);
        break;
    }
    let Some(offset) = chosen_offset else {
        return false;
    };

    let modifiers = report[offset];
    let keys = &report[offset + 2..offset + 8];
    if strict_usage_check
        && keys
            .iter()
            .copied()
            .any(|usage| usage != 0 && map_hid_usage_to_set1_scancode(usage).is_none())
    {
        return false;
    }
    inject_modifier_transitions(modifiers, state);

    let prev_keys = state.prev_keys;
    for &usage in &prev_keys {
        if usage == 0 || keys.contains(&usage) {
            continue;
        }
        if let Some(scancode) = map_hid_usage_to_set1_scancode(usage) {
            inject_scancode(scancode | SC_RELEASE, state);
        }
    }

    for &usage in keys {
        if usage == 0 || prev_keys.contains(&usage) {
            continue;
        }
        if let Some(scancode) = map_hid_usage_to_set1_scancode(usage) {
            inject_scancode(scancode, state);
        }
    }

    state.prev_keys.copy_from_slice(keys);
    true
}

fn parse_hid_mouse_report(
    slot: u8,
    ep: u8,
    report: &[u8],
    state: &mut HidParserState,
) -> bool {
    if report.len() < 3 {
        return false;
    }

    let mut inject_errno: Option<i64> = None;
    let raw_buttons = report[0];
    if (raw_buttons & 0xE0) != 0 {
        return false;
    }
    let buttons = raw_buttons & 0x07;
    let dx = report[1] as i8;
    let dy = report[2] as i8;
    let wheel = if report.len() > 3 { report[3] as i8 } else { 0 };

    let prev_buttons = mouse_entry_mut(state, slot, ep).prev_buttons;
    let has_change = dx != 0 || dy != 0 || wheel != 0 || buttons != prev_buttons;
    if has_change {
        if let Err(err) = input::inject_mouse_packet(buttons, dx, dy, wheel) {
            inject_errno = Some(err as i64);
        }
    }
    mouse_entry_mut(state, slot, ep).prev_buttons = buttons;
    if let Some(errno) = inject_errno {
        if !state.warned_mouse_inject {
            println!("[xHCI] mouse inject failed: errno={}", errno);
            state.warned_mouse_inject = true;
        }
    }
    true
}

pub fn parse_hid_report(
    slot: u8,
    ep: u8,
    report: &[u8],
    kind: HidReportKind,
    state: &mut HidParserState,
) {
    match kind {
        HidReportKind::Keyboard => {
            let _ = parse_hid_keyboard_report(slot, ep, report, state, false);
        }
        HidReportKind::Mouse => {
            let _ = parse_hid_mouse_report(slot, ep, report, state);
        }
        HidReportKind::Unknown => {
            if parse_hid_keyboard_report(slot, ep, report, state, true) {
                return;
            }
            // Unknown はここではマウス扱いしない。
            // 種別確定は列挙時（descriptor 解析）で行う。
        }
    }
}
