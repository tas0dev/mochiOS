//! MMIO/物理メモリマップ関連システムコール

use crate::sys::{syscall1, syscall2, SyscallNumber};

const EINVAL: u64 = (-22i64) as u64;

/// 物理アドレス範囲を現在プロセスのユーザー空間にマップする
///
/// 成功時はマップされた先頭仮想アドレスを返す。
pub fn map_physical(phys_addr: u64, size: usize) -> Result<*mut u8, u64> {
    if size == 0 {
        return Err(EINVAL);
    }

    let ret = syscall2(SyscallNumber::MapPhysicalRange as u64, phys_addr, size as u64);
    if ret == 0 || (ret as i64) < 0 {
        Err(ret)
    } else {
        Ok(ret as *mut u8)
    }
}

/// ユーザー仮想アドレスを物理アドレスへ変換する
pub fn virt_to_phys(ptr: *const u8) -> Result<u64, u64> {
    let ret = syscall1(SyscallNumber::VirtToPhys as u64, ptr as u64);
    if ret == 0 || (ret as i64) < 0 {
        Err(ret)
    } else {
        Ok(ret)
    }
}
