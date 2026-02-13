//! ユーザー側システムコール共通部

use core::arch::asm;

/// システムコール番号
#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallNumber {
    /// スケジューラへ譲る
    Yield = 1,
    /// タイマーティック数を取得
    GetTicks = 2,
    /// IPC送信
    IpcSend = 3,
    /// IPC受信
    IpcRecv = 4,
    /// Exec
    Exec = 5,
    /// プロセス終了
    Exit = 6,
    /// 書き込み
    Write = 7,
    /// 読み込み
    Read = 8,
    /// 現在のプロセスIDを取得
    GetPid = 9,
    /// 現在のスレッドIDを取得
    GetTid = 10,
    /// スリープ
    Sleep = 11,
    /// ファイルを開く
    Open = 12,
    /// ファイルを閉じる
    Close = 13,
    /// Fork
    Fork = 14,
    /// Wait
    Wait = 15,
    /// メモリブレーク
    Brk = 16,
    /// ファイルシーク
    Lseek = 17,
    /// ファイル情報取得
    Fstat = 18,
    /// 名前からプロセスIDを検索
    FindProcessByName = 19,
    /// カーネルログを出力
    Log = 20,
    /// I/Oポート入力
    PortIn = 21,
    /// I/Oポート出力
    PortOut = 22,
}

#[inline(always)]
pub(crate) fn syscall0(num: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") num => ret,
            options(nostack, preserves_flags)
        );
    }
    ret
}

#[inline(always)]
pub(crate) fn syscall1(num: u64, arg0: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") num => ret,
            in("rdi") arg0,
            options(nostack, preserves_flags)
        );
    }
    ret
}

#[inline(always)]
pub(crate) fn syscall2(num: u64, arg0: u64, arg1: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") num => ret,
            in("rdi") arg0,
            in("rsi") arg1,
            options(nostack, preserves_flags)
        );
    }
    ret
}

#[inline(always)]
#[allow(dead_code)]
pub(crate) fn syscall3(num: u64, arg0: u64, arg1: u64, arg2: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") num => ret,
            in("rdi") arg0,
            in("rsi") arg1,
            in("rdx") arg2,
            options(nostack, preserves_flags)
        );
    }
    ret
}

#[inline(always)]
#[allow(dead_code)]
pub(crate) fn syscall4(num: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") num => ret,
            in("rdi") arg0,
            in("rsi") arg1,
            in("rdx") arg2,
            in("r10") arg3,
            options(nostack, preserves_flags)
        );
    }
    ret
}

#[inline(always)]
#[allow(dead_code)]
pub(crate) fn syscall5(num: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64) -> u64 {
    let ret: u64;
    unsafe {
        asm!(
            "int 0x80",
            inlateout("rax") num => ret,
            in("rdi") arg0,
            in("rsi") arg1,
            in("rdx") arg2,
            in("r10") arg3,
            in("r8") arg4,
            options(nostack, preserves_flags)
        );
    }
    ret
}
