//! ユーザー側システムコール共通部

use core::arch::asm;

/// システムコール番号 (Linux x86_64 互換)
#[repr(u64)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallNumber {
    /// 読み込み
    Read = 0,
    /// 書き込み
    Write = 1,
    /// ファイルを開く
    Open = 2,
    /// ファイルを閉じる
    Close = 3,
    /// ファイル情報取得
    Fstat = 5,
    /// ファイルシーク
    Lseek = 8,
    /// メモリマップ
    Mmap = 9,
    /// メモリアンマップ
    Munmap = 11,
    /// メモリブレーク
    Brk = 12,
    /// シグナル処理（スタブ）
    RtSigaction = 13,
    /// シグナルマスク（スタブ）
    RtSigprocmask = 14,
    /// Fork
    Fork = 57,
    /// プロセス終了
    Exit = 60,
    /// Wait
    Wait = 61,
    /// 現在のプロセスIDを取得
    GetPid = 39,
    /// 現在のスレッドIDを取得
    GetTid = 186,
    /// clone (スレッド生成)
    Clone = 56,
    /// arch_prctl (TLS用FSベース設定)
    ArchPrctl = 158,
    /// clock_gettime
    ClockGettime = 228,
    /// futex
    Futex = 202,
    /// exit_group
    ExitGroup = 231,
    /// getcwd
    Getcwd = 79,

    // SwiftCore独自syscall (Linux未使用番号帯を使用: 512+)
    /// スケジューラへ譲る
    Yield = 512,
    /// タイマーティック数を取得
    GetTicks = 513,
    /// IPC送信
    IpcSend = 514,
    /// IPC受信
    IpcRecv = 515,
    /// initfsから実行可能ファイルを実行
    Exec = 516,
    /// スリープ (ms単位)
    Sleep = 517,
    /// 名前からプロセスIDを検索
    FindProcessByName = 518,
    /// カーネルログを出力
    Log = 519,
    /// I/Oポート入力
    PortIn = 520,
    /// I/Oポート出力
    PortOut = 521,
    /// ディレクトリ作成
    Mkdir = 522,
    /// ディレクトリ削除
    Rmdir = 523,
    /// ディレクトリエントリ読み取り
    Readdir = 524,
    /// カレントディレクトリ変更
    Chdir = 525,
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

#[inline(always)]
#[allow(dead_code)]
pub(crate) fn syscall6(
    num: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64,
) -> u64 {
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
            in("r9") arg5,
            options(nostack, preserves_flags)
        );
    }
    ret
}

