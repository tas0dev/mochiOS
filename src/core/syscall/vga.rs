//! フレームバッファ関連のシステムコール

use super::types::{EFAULT, EINVAL, ENOMEM, SUCCESS};

/// ユーザー空間に返すフレームバッファ情報構造体のレイアウト
///
/// ```text
/// offset  size  field
///   0       4   width   (ピクセル単位)
///   4       4   height  (ピクセル単位)
///   8       4   stride  (1行あたりの u32 ピクセル数)
///  12       4   _pad
/// ```
const FB_INFO_SIZE: u64 = 16;

/// フレームバッファ情報をユーザー空間の構造体に書き込む
///
/// # Arguments
/// * `info_ptr` - `FbInfo` 構造体へのユーザー空間ポインタ
///
/// # Returns
/// 成功時は `SUCCESS`、失敗時はエラーコード
pub fn get_framebuffer_info(info_ptr: u64) -> u64 {
    if info_ptr == 0 {
        return EFAULT;
    }
    if !crate::syscall::validate_user_ptr(info_ptr, FB_INFO_SIZE) {
        return EFAULT;
    }

    let fb_info = match crate::util::vga::get_info() {
        Some(i) => i,
        None => return EINVAL,
    };

    crate::syscall::with_user_memory_access(|| unsafe {
        let ptr = info_ptr as *mut u32;
        ptr.add(0).write_volatile(fb_info.width as u32);
        ptr.add(1).write_volatile(fb_info.height as u32);
        ptr.add(2).write_volatile(fb_info.stride as u32);
        ptr.add(3).write_volatile(0u32);
    });

    SUCCESS
}

/// フレームバッファ物理メモリを呼び出し元プロセスのアドレス空間にマップする
///
/// # Returns
/// マップされた仮想アドレス、または失敗時はエラーコード
pub fn map_framebuffer() -> u64 {
    let fb_info = match crate::util::vga::get_info() {
        Some(i) => i,
        None => return EINVAL,
    };

    let phys_addr = fb_info.addr;
    let phys_base = phys_addr & !0xfffu64;
    let phys_offset = phys_addr & 0xfffu64;
    // stride は u32 ピクセル単位、1ピクセル = 4バイト
    let fb_size = match (fb_info.height as u64)
        .checked_mul(fb_info.stride as u64)
        .and_then(|v| v.checked_mul(4))
    {
        Some(v) => v,
        None => return EINVAL,
    };
    // 先頭の物理オフセット分も含めてページ境界まで拡張する
    let map_size = fb_size
        .checked_add(phys_offset)
        .and_then(|v| v.checked_add(0xfff))
        .map(|v| v & !0xfffu64)
        .unwrap_or(0);

    if map_size == 0 {
        return EINVAL;
    }

    let tid = match crate::task::current_thread_id() {
        Some(t) => t,
        None => return ENOMEM,
    };
    let pid = match crate::task::with_thread(tid, |t| t.process_id()) {
        Some(p) => p,
        None => return ENOMEM,
    };

    let result = crate::task::with_process_mut(pid, |process| {
        // mmap ベースアドレスとして heap_end を流用する
        if process.heap_start() == 0 {
            let default_base = 0x5000_0000u64;
            process.set_heap_start(default_base);
            process.set_heap_end(default_base);
        }

        let base = process.heap_end();
        let map_start = base.checked_add(0xfff).map(|v| v & !0xfffu64).unwrap_or(0);
        if map_start == 0 || map_start > 0x0000_7FFF_FFFF_FFFF {
            return Err(ENOMEM);
        }

        let pt_phys = match process.page_table() {
            Some(p) => p,
            None => return Err(ENOMEM),
        };

        crate::mem::paging::map_physical_range_to_user(pt_phys, map_start, phys_base, map_size)
            .map_err(|_| ENOMEM)?;

        let new_end = map_start.checked_add(map_size).unwrap_or(map_start);
        process.set_heap_end(new_end);

        Ok(map_start + phys_offset)
    });

    match result {
        Some(Ok(va)) => va,
        Some(Err(e)) => e,
        None => ENOMEM,
    }
}
