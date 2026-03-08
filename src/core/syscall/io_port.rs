//! I/Oポートアクセス用のシステムコール

use crate::syscall::{EINVAL, EPERM, SUCCESS};
use core::arch::asm;

/// 呼び出し元プロセスがI/Oポートアクセス権限を持つか確認する
///
/// ServiceまたはCore権限レベルのプロセスのみ許可する
fn caller_has_port_privilege() -> bool {
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

/// I/Oポートから読み取り
///
/// # Arguments
/// * `port` - ポート番号 (0-65535)
/// * `size` - データサイズ (1=byte, 2=word, 4=dword)
///
/// # Returns
/// 読み取った値、またはエラー時は EINVAL
pub fn port_in(port: u64, size: u64) -> u64 {
    // 権限チェック: ServiceまたはCore権限のプロセスのみI/Oポートアクセスを許可
    if !caller_has_port_privilege() {
        return EPERM;
    }

    if port > 0xFFFF {
        return EINVAL;
    }

    let port = port as u16;

    unsafe {
        match size {
            1 => {
                // inb
                let value: u8;
                asm!(
                    "in al, dx",
                    in("dx") port,
                    out("al") value,
                    options(nomem, nostack, preserves_flags)
                );
                value as u64
            }
            2 => {
                // inw
                let value: u16;
                asm!(
                    "in ax, dx",
                    in("dx") port,
                    out("ax") value,
                    options(nomem, nostack, preserves_flags)
                );
                value as u64
            }
            4 => {
                // inl
                let value: u32;
                asm!(
                    "in eax, dx",
                    in("dx") port,
                    out("eax") value,
                    options(nomem, nostack, preserves_flags)
                );
                value as u64
            }
            _ => EINVAL,
        }
    }
}

/// I/Oポートへ書き込み
///
/// # Arguments
/// * `port` - ポート番号 (0-65535)
/// * `value` - 書き込む値
/// * `size` - データサイズ (1=byte, 2=word, 4=dword)
///
/// # Returns
/// SUCCESS、またはエラー時は EINVAL
pub fn port_out(port: u64, value: u64, size: u64) -> u64 {
    // 権限チェック: ServiceまたはCore権限のプロセスのみI/Oポートアクセスを許可
    if !caller_has_port_privilege() {
        return EPERM;
    }

    if port > 0xFFFF {
        return EINVAL;
    }

    let port = port as u16;

    unsafe {
        match size {
            1 => {
                // outb
                asm!(
                    "out dx, al",
                    in("dx") port,
                    in("al") value as u8,
                    options(nomem, nostack, preserves_flags)
                );
            }
            2 => {
                // outw
                asm!(
                    "out dx, ax",
                    in("dx") port,
                    in("ax") value as u16,
                    options(nomem, nostack, preserves_flags)
                );
            }
            4 => {
                // outl
                asm!(
                    "out dx, eax",
                    in("dx") port,
                    in("eax") value as u32,
                    options(nomem, nostack, preserves_flags)
                );
            }
            _ => return EINVAL,
        }
    }

    SUCCESS
}
