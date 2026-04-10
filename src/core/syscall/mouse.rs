use crate::syscall::{EINVAL, ENODATA, EPERM, SUCCESS};

/// マウス入力注入 API を呼び出せるか確認する
///
/// Service または Core 権限のみ許可する。
fn caller_has_mouse_inject_privilege() -> bool {
    crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
        .and_then(|pid| {
            crate::task::with_process(pid, |p| {
                matches!(
                    p.privilege(),
                    crate::task::PrivilegeLevel::Core | crate::task::PrivilegeLevel::Service
                )
            })
        })
        .unwrap_or(false)
}

/// PS/2 マウスパケットを 1 つ読み取る（非ブロッキング）
///
/// 返り値は `b0 | (b1 << 8) | (b2 << 16)` 形式。
/// キューが空なら ENODATA。
pub fn read_packet() -> Result<u64, u64> {
    match crate::util::ps2mouse::pop_packet() {
        Some(packet) => Ok(packet as u64),
        None => Err(ENODATA),
    }
}

/// PS/2 マウスパケットを 1 つ読み取る（ブロッキング）
///
/// データが到着するまで waiter 登録してスレッドをスリープし、
/// IRQ12 での wake により再開する。
pub fn read_packet_blocking() -> Result<u64, u64> {
    if let Some(packet) = crate::util::ps2mouse::pop_packet() {
        return Ok(packet as u64);
    }

    let tid = match crate::task::current_thread_id() {
        Some(id) => id,
        None => loop {
            if let Some(packet) = crate::util::ps2mouse::pop_packet() {
                return Ok(packet as u64);
            }
            crate::task::yield_now();
        },
    };

    loop {
        if crate::util::ps2mouse::register_waiter(tid.as_u64()) {
            if let Some(packet) = crate::util::ps2mouse::pop_packet() {
                crate::util::ps2mouse::unregister_waiter(tid.as_u64());
                return Ok(packet as u64);
            }

            if crate::task::sleep_thread_unless_woken(tid) {
                crate::task::yield_now();
            }
        } else {
            if let Some(packet) = crate::util::ps2mouse::pop_packet() {
                return Ok(packet as u64);
            }
            crate::task::yield_now();
        }
    }
}

/// 3バイト相当のマウスパケットを通常入力キューへ注入する（Service/Core専用）
///
/// `packet` は `b0 | (b1 << 8) | (b2 << 16)` 形式。
pub fn inject_packet(packet: u64) -> u64 {
    if !caller_has_mouse_inject_privilege() {
        return EPERM;
    }
    if packet > 0xFF_FFFF {
        return EINVAL;
    }
    let b0 = (packet & 0xFF) as u8;
    let b1 = ((packet >> 8) & 0xFF) as u8;
    let b2 = ((packet >> 16) & 0xFF) as u8;
    // caller が buttons のみ渡した場合でもパケット同期できるよう補完
    let mut status = b0;
    if (b0 & 0x08) == 0 {
        status |= 0x08;
        status &= !((1 << 4) | (1 << 5));
        if (b1 & 0x80) != 0 {
            status |= 1 << 4;
        }
        if (b2 & 0x80) != 0 {
            status |= 1 << 5;
        }
    }
    let packet32 = u32::from(status) | (u32::from(b1) << 8) | (u32::from(b2) << 16);
    crate::util::ps2mouse::push_packet(packet32);
    SUCCESS
}
