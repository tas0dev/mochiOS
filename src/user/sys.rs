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
    /// ベクタ書き込み
    Writev = 20,
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
    /// シグナルリターン
    RtSigreturn = 15,
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
    /// kill (シグナルを送る)
    Kill = 62,
    /// getcwd
    Getcwd = 79,

    // mochiOS独自syscall (Linux未使用番号帯を使用: 512+)
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
    /// キーボードから1文字読み取る（ユーザー側）
    KeyboardRead = 526,
/// スレッドIDからプロセスの権限レベルを取得 (0=Core, 1=Service, 2=User)
    GetThreadPrivilege = 527,
    /// フレームバッファ情報取得
    GetFramebufferInfo = 528,
    /// フレームバッファをマップ
    MapFramebuffer = 529,
    /// メモリ上の ELF バッファから新プロセスを起動
    ExecFromBuffer = 530,
    /// コンソールカーソルのピクセルY位置を設定
    SetConsoleCursor = 531,
    /// コンソールカーソルのピクセルY位置を取得
    GetConsoleCursor = 532,
    /// IPC受信（ブロッキング版）
    IpcRecvWait = 533,
    /// キーボード入力の監視用タップ（通常入力を消費しない）
    KeyboardReadTap = 534,
    /// PS/2 マウスの3バイトパケットを読み取る（b0 | b1<<8 | b2<<16）
    MouseRead = 535,
    /// 物理アドレス範囲をユーザー空間にマップ
    MapPhysicalRange = 536,
    /// ユーザー仮想アドレスを物理アドレスへ変換
    VirtToPhys = 537,
    /// I/Oポートから 16-bit ワード列を一括読み取り
    PortInWords = 538,
    /// I/Oポートへ 16-bit ワード列を一括書き込み
    PortOutWords = 539,
    /// キーボード入力キューへ raw スキャンコードを注入
    KeyboardInject = 540,
    /// マウス入力キューへ 3バイトパケットを注入
    MouseInject = 541,
    /// メモリ上の ELF バッファと実行パス名から新プロセスを起動
    ExecFromBufferNamed = 542,
    /// メモリ上の ELF バッファと実行パス名＋引数から新プロセスを起動
    ExecFromBufferNamedArgs = 543,
    /// メモリ上の ELF バッファと実行パス名＋引数＋要求元スレッドIDから新プロセスを起動
    ExecFromBufferNamedArgsWithRequester = 544,
    /// FS 経由のストリーム exec（マップ書き込みを試行）
    ExecFromFsStream = 545,
}

/// 操作が許可されていない
pub const EPERM: u64 = (-1i64) as u64;
/// 無効な引数
pub const EINVAL: u64 = (-22i64) as u64;
/// 入力が空
pub const ENODATA: u64 = (-61i64) as u64;

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
