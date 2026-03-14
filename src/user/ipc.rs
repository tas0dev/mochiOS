//! IPC 系システムコール（ユーザー側）

use super::sys::{syscall2, syscall3, SyscallNumber};

/// IPC送信（宛先スレッドID, データ）
pub fn ipc_send(dest_thread_id: u64, data: &[u8]) -> u64 {
    syscall3(
        SyscallNumber::IpcSend as u64,
        dest_thread_id,
        data.as_ptr() as u64,
        data.len() as u64,
    )
}

/// IPC受信（ノンブロッキング）
/// 戻り値: (sender_id, received_len) — メッセージがなければ (0, 0)
pub fn ipc_recv(buf: &mut [u8]) -> (u64, u64) {
    let ret = syscall2(
        SyscallNumber::IpcRecv as u64,
        buf.as_mut_ptr() as u64,
        buf.len() as u64
    );
    // EAGAIN やその他エラーは負の i64 として返る
    if (ret as i64) < 0 {
        return (0, 0);
    }
    // カーネルは (sender << 32 | len) を返す
    let sender = ret >> 32;
    let len = ret & 0xFFFF_FFFF;
    (sender, len)
}

/// IPC受信（ブロッキング版）
/// メッセージが届くまでスリープして待機する。
/// 戻り値: (sender_id, received_len)
pub fn ipc_recv_wait(buf: &mut [u8]) -> (u64, u64) {
    let ret = syscall2(
        SyscallNumber::IpcRecvWait as u64,
        buf.as_mut_ptr() as u64,
        buf.len() as u64,
    );
    if (ret as i64) < 0 {
        return (0, 0);
    }
    let sender = ret >> 32;
    let len = ret & 0xFFFF_FFFF;
    (sender, len)
}
