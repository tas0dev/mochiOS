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
    /// ファイル情報取得 (path)
    Stat = 4,
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
    /// clone (スレッド生成)
    Clone = 56,
    /// Fork
    Fork = 57,
    /// Execve
    Execve = 59,
    /// Wait
    Wait = 61,
    /// 現在のプロセスIDを取得
    GetPid = 39,
    /// 現在のスレッドIDを取得
    GetTid = 186,
    /// arch_prctl (TLS用FSベース設定)
    ArchPrctl = 158,
    /// clock_gettime
    ClockGettime = 228,
    /// futex
    Futex = 202,
    /// プロセス終了
    Exit = 60,
    /// exit_group (全スレッドを終了)
    ExitGroup = 231,
    /// kill (シグナルを送る)
    Kill = 62,
    /// getcwd
    Getcwd = 79,
    /// getppid
    GetPpid = 110,
    /// setpgid
    Setpgid = 109,
    /// getpgid
    Getpgid = 121,
    /// setsid
    Setsid = 112,
    /// getsid
    Getsid = 124,
    /// ioctl
    Ioctl = 16,
    /// access
    Access = 21,
    /// getuid
    Getuid = 102,
    /// getgid
    Getgid = 104,
    /// geteuid
    Geteuid = 107,
    /// getegid
    Getegid = 108,
    /// lstat (stat のシンボリックリンク非追跡版、ここでは stat と同一実装)
    Lstat = 6,
    /// readlink (スタブ)
    Readlink = 89,
    /// fcntl (FD フラグ操作)
    Fcntl = 72,
    /// pipe
    Pipe = 22,
    /// dup
    Dup = 32,
    /// dup2
    Dup2 = 33,
    /// mprotect
    Mprotect = 10,
    /// nanosleep
    Nanosleep = 35,
    /// uname
    Uname = 63,
    /// getrlimit
    Getrlimit = 97,
    /// set_tid_address
    SetTidAddress = 218,
    /// openat
    Openat = 257,
    /// getdents64
    Getdents64 = 217,
    /// prlimit64
    Prlimit64 = 302,
    /// pipe2
    Pipe2 = 293,
    /// newfstatat (fstatat)
    Newfstatat = 262,
    /// faccessat
    Faccessat = 269,
    /// readlinkat
    Readlinkat = 267,

    // mochiOS独自syscall (Linux未使用番号帯: 512+)
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
    /// キーボードから1文字読み取る（mochiOS 固有）
    KeyboardRead = 526,
    /// スレッドIDからプロセスの権限レベルを取得 (0=Core, 1=Service, 2=User)
    GetThreadPrivilege = 527,
    /// フレームバッファ情報を取得 (info_ptr: *mut FbInfo)
    GetFramebufferInfo = 528,
    /// フレームバッファをユーザー空間にマップ、マップ済み仮想アドレスを返す
    MapFramebuffer = 529,
    /// メモリ上の ELF バッファから新プロセスを起動
    ExecFromBuffer = 530,
    /// コンソールカーソルのピクセルY位置を設定
    SetConsoleCursor = 531,
    /// コンソールカーソルのピクセルY位置を取得
    GetConsoleCursor = 532,
    /// IPC受信（ブロッキング版）：メッセージが届くまでスリープして待機
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
    /// キーボード入力キューへ raw スキャンコードを注入（Service/Core専用）
    KeyboardInject = 540,
    /// マウス入力キューへ 3バイトパケットを注入（Service/Core専用）
    MouseInject = 541,
    /// メモリ上の ELF バッファと実行パス名から新プロセスを起動
    ExecFromBufferNamed = 542,
    /// メモリ上の ELF バッファと実行パス名＋引数から新プロセスを起動
    ExecFromBufferNamedArgs = 543,
    /// メモリ上の ELF バッファと実行パス名＋引数＋要求元スレッドIDから新プロセスを起動
    ExecFromBufferNamedArgsWithRequester = 544,
    /// Execute by streaming ELF image into kernel (path_ptr, args_ptr)
    ExecFromFsStream = 545,
    /// 物理ページ配列をターゲットプロセスのアドレス空間にマップ（Service権限専用）
    MapPhysicalPages = 546,
    /// 仮想アドレスから物理アドレスを取得（Service権限専用）
    GetPhysicalAddr = 547,
    /// 共有用物理ページを割り当て、自プロセスにマップして物理アドレスを返す（Service権限専用）
    AllocSharedPages = 548,
    /// 物理ページをアンマップして解放（Service権限専用）
    UnmapPages = 549,
    /// IPC経由で物理ページをターゲットプロセスへ送信（Service権限専用）
    IpcSendPages = 550,
    /// PS/2 マウスの3バイトパケットを読み取る（ブロッキング版）
    MouseReadWait = 551,
}

/// 成功
pub const SUCCESS: u64 = 0;

// Linux互換エラーコード（負の値を u64 にキャストして返す）
/// 操作が許可されていない
pub const EPERM: u64 = (-1i64) as u64;
/// ファイルが見つからない
pub const ENOENT: u64 = (-2i64) as u64;
/// ディレクトリではない
pub const ENOTDIR: u64 = (-20i64) as u64;
/// プロセスが見つからない
pub const ESRCH: u64 = (-3i64) as u64;
/// I/Oエラー
pub const EIO: u64 = (-5i64) as u64;
/// 不正なファイルディスクリプタ
pub const EBADF: u64 = (-9i64) as u64;
/// 不正なアドレス
pub const EFAULT: u64 = (-14i64) as u64;
/// デバイスが見つからない
pub const ENXIO: u64 = (-6i64) as u64;
/// 無効な引数
pub const EINVAL: u64 = (-22i64) as u64;
/// 未実装
pub const ENOSYS: u64 = (-38i64) as u64;
/// データがない / ノンブロッキングで読み出しできない
pub const ENODATA: u64 = (-61i64) as u64;
/// 受信/送信できない（キュー空/満杯）
pub const EAGAIN: u64 = (-11i64) as u64;
/// メモリ不足
pub const ENOMEM: u64 = (-12i64) as u64;
/// ファイルが既に存在する
pub const EEXIST: u64 = (-17i64) as u64;
/// デバイスでない (TTY 操作に非 TTY FD を使用した)
pub const ENOTTY: u64 = (-25i64) as u64;
/// 引数が範囲外
pub const ERANGE: u64 = (-34i64) as u64;
/// 操作がサポートされていない
pub const ENOTSUP: u64 = (-95i64) as u64;
/// パイプが壊れている
pub const EPIPE: u64 = (-32i64) as u64;
/// ファイルディスクリプタが多すぎる
pub const EMFILE: u64 = (-24i64) as u64;
