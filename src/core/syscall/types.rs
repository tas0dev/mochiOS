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
    /// clone (スレッド生成)
    Clone = 56,
    /// Fork
    Fork = 57,
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
    /// getcwd
    Getcwd = 79,

    // SwiftCore独自syscall (Linux未使用番号帯: 512+)
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

/// 成功
pub const SUCCESS: u64 = 0;

// Linux互換エラーコード（負の値を u64 にキャストして返す）
/// 操作が許可されていない
pub const EPERM: u64 = (-1i64) as u64;
/// ファイルが見つからない
pub const ENOENT: u64 = (-2i64) as u64;
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
/// 受信/送信できない（キュー空/満杯）
pub const EAGAIN: u64 = (-11i64) as u64;
/// メモリ不足
pub const ENOMEM: u64 = (-12i64) as u64;

