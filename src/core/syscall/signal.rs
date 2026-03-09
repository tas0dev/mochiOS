//! シグナル関連のシステムコール
//!
//! rt_sigaction / rt_sigprocmask / kill / rt_sigreturn と、
//! syscall リターン時のシグナル送達ロジックを実装する。

use super::types::{EINVAL, EPERM, ESRCH, SUCCESS};
use crate::task::{
    current_thread_id, default_action, with_process, with_process_mut, DefaultAction,
    ProcessId, SigAction, SIG_DFL, SIG_IGN, SIGCHLD, SIGKILL,
};

// ---- rt_sigprocmask の how 引数 ----
const SIG_BLOCK:   u64 = 0;
const SIG_UNBLOCK: u64 = 1;
const SIG_SETMASK: u64 = 2;

// ---- SIGKILL / SIGSTOP はブロック・ハンドラ変更不可 ----
const UNCATCHABLE_MASK: u64 = (1u64 << (SIGKILL - 1)) | (1u64 << (18usize));  // SIGKILL | SIGSTOP

// ---- ユーザー空間の struct sigaction レイアウト (Linux x86-64 互換) ----
// sa_handler:  [+0]  u64
// sa_flags:    [+8]  u64
// sa_restorer: [+16] u64
// sa_mask:     [+24] u64  (128-bit mask, 上位64bitは今回使わない)
const SIGACTION_SIZE: u64 = 32;

/// rt_sigaction システムコール
///
/// # 引数
/// - `signum`: シグナル番号 (1–64)
/// - `new_act_ptr`: 新しいアクション (NULLなら変更しない)
/// - `old_act_ptr`: 旧アクションの書き出し先 (NULLなら無視)
pub fn rt_sigaction(signum: u64, new_act_ptr: u64, old_act_ptr: u64) -> u64 {
    let sig = signum as usize;
    if sig < 1 || sig > 64 {
        return EINVAL;
    }
    // SIGKILL/SIGSTOP のハンドラは変更不可
    if (new_act_ptr != 0) && ((1u64 << (sig - 1)) & UNCATCHABLE_MASK != 0) {
        return EINVAL;
    }

    let pid = match current_pid() { Some(p) => p, None => return EINVAL };

    // 旧アクションを読み出してユーザー空間に書く
    if old_act_ptr != 0 {
        if !crate::syscall::validate_user_ptr(old_act_ptr, SIGACTION_SIZE) {
            return super::types::EFAULT;
        }
        let old = with_process(pid, |p| p.signal_state().action(sig))
            .unwrap_or(SigAction::default_action());
        crate::syscall::with_user_memory_access(|| unsafe {
            let p = old_act_ptr as *mut u64;
            p.add(0).write(old.handler);
            p.add(1).write(old.flags);
            p.add(2).write(old.restorer);
            p.add(3).write(old.mask);
        });
    }

    // 新アクションをカーネルに保存
    if new_act_ptr != 0 {
        if !crate::syscall::validate_user_ptr(new_act_ptr, SIGACTION_SIZE) {
            return super::types::EFAULT;
        }
        let (handler, flags, restorer, mask) =
            crate::syscall::with_user_memory_access(|| unsafe {
                let p = new_act_ptr as *const u64;
                (p.add(0).read(), p.add(1).read(), p.add(2).read(), p.add(3).read())
            });
        // mask の uncatchable ビットは強制クリア
        let mask = mask & !UNCATCHABLE_MASK;
        let action = SigAction { handler, flags, restorer, mask };
        with_process_mut(pid, |p| p.signal_state_mut().set_action(sig, action));
    }

    SUCCESS
}

/// rt_sigprocmask システムコール
///
/// # 引数
/// - `how`: SIG_BLOCK / SIG_UNBLOCK / SIG_SETMASK
/// - `set_ptr`: 操作対象マスク (NULLなら変更しない)
/// - `oldset_ptr`: 旧マスクの書き出し先 (NULLなら無視)
pub fn rt_sigprocmask(how: u64, set_ptr: u64, oldset_ptr: u64) -> u64 {
    let pid = match current_pid() { Some(p) => p, None => return EINVAL };

    // 旧マスクを返す
    if oldset_ptr != 0 {
        if !crate::syscall::validate_user_ptr(oldset_ptr, 8) {
            return super::types::EFAULT;
        }
        let old_mask = with_process(pid, |p| p.signal_state().mask).unwrap_or(0);
        crate::syscall::with_user_memory_access(|| unsafe {
            (oldset_ptr as *mut u64).write(old_mask);
        });
    }

    if set_ptr == 0 {
        return SUCCESS;
    }
    if !crate::syscall::validate_user_ptr(set_ptr, 8) {
        return super::types::EFAULT;
    }
    let new_set = crate::syscall::with_user_memory_access(|| unsafe {
        (set_ptr as *const u64).read()
    }) & !UNCATCHABLE_MASK;  // SIGKILL/SIGSTOP は常にアンブロック

    with_process_mut(pid, |p| {
        let mask = &mut p.signal_state_mut().mask;
        match how {
            SIG_BLOCK   => *mask |= new_set,
            SIG_UNBLOCK => *mask &= !new_set,
            SIG_SETMASK => *mask  = new_set,
            _ => {}
        }
    });

    SUCCESS
}

/// kill システムコール
///
/// # 引数
/// - `pid_raw`: ターゲット PID (正数=指定PID, 0=同じプロセスグループ, -1=すべて)
/// - `sig_raw`: シグナル番号 (0=存在確認のみ)
pub fn kill(pid_raw: u64, sig_raw: u64) -> u64 {
    let target_pid_raw = pid_raw as i64;
    let sig = sig_raw as usize;

    if sig > 64 {
        return EINVAL;
    }

    // sig == 0: プロセス存在確認のみ
    if sig == 0 {
        let exists = if target_pid_raw > 0 {
            let target = ProcessId::from_u64(target_pid_raw as u64);
            with_process(target, |_| ()).is_some()
        } else {
            true
        };
        return if exists { SUCCESS } else { ESRCH };
    }

    if target_pid_raw > 0 {
        deliver_signal_to_pid(ProcessId::from_u64(target_pid_raw as u64), sig)
    } else if target_pid_raw == -1 {
        // 全プロセス（カーネル除く）に送る
        let mut found = false;
        let current_pid = current_pid();
        let mut pids = alloc::vec::Vec::new();
        crate::task::for_each_process(|p| pids.push(p.id()));
        for pid in pids {
            if Some(pid) != current_pid {
                if deliver_signal_to_pid(pid, sig) == SUCCESS {
                    found = true;
                }
            }
        }
        if found { SUCCESS } else { ESRCH }
    } else {
        EINVAL
    }
}

/// 指定プロセスにシグナルを送達する（カーネル内部からも呼ばれる）
pub fn deliver_signal_to_pid(pid: ProcessId, sig: usize) -> u64 {
    if sig < 1 || sig > 64 {
        return EINVAL;
    }

    let exists = with_process(pid, |_| ()).is_some();
    if !exists {
        return ESRCH;
    }

    // SIGKILL はハンドラを無視して即終了
    if sig == SIGKILL {
        kill_process_immediately(pid, sig as u64);
        return SUCCESS;
    }

    // pending ビットをセット
    with_process_mut(pid, |p| p.signal_state_mut().set_pending(sig));

    // ブロッキング待機しているスレッドを起床させる（シグナルを受け取れるよう）
    wake_first_thread_of(pid);

    SUCCESS
}

/// 子プロセス終了時に親プロセスへ SIGCHLD を送達する（scheduler から呼ばれる）
pub fn deliver_sigchld_to_parent(child_pid: ProcessId) {
    let parent_pid = match with_process(child_pid, |p| p.parent_id()) {
        Some(Some(pid)) => pid,
        _ => return,
    };
    deliver_signal_to_pid(parent_pid, SIGCHLD);
}

// ---- syscall リターン時のシグナル送達 ----------------------------------------

/// int 0x80 リターン時に呼ばれる: pending シグナルの送達とシグナルフレームの設定
///
/// # 引数
/// - `kstack`: int 0x80 ハンドラが積んだレジスタ保存領域の先頭ポインタ
/// - `syscall_ret`: syscall の戻り値
///
/// # Returns
/// 最終的な syscall 戻り値（シグナル送達時は変更される場合がある）
///
/// # kstack レイアウト（push 順の逆、低アドレスが先頭）
/// ```
/// [0]  r15, [1]  r14, [2]  r13, [3]  r12,
/// [4]  r11, [5]  r10, [6]  r9,  [7]  r8,
/// [8]  rdi (arg0),   [9]  rsi,  [10] rbp, [11] rbx,
/// [12] rdx,          [13] rcx,  [14] rax (syscall number),
/// --- CPU 割り込みフレーム ---
/// [15] user RIP, [16] user CS, [17] user RFLAGS, [18] user RSP, [19] user SS
/// ```
#[no_mangle]
pub extern "sysv64" fn signal_and_return(kstack: *mut u64, syscall_ret: u64) -> u64 {
    // kstack[14] = [rsp+112] = 元の syscall 番号（dispatch 呼び出し前の push rax）
    let syscall_num = unsafe { kstack.add(14).read() };

    // rt_sigreturn (15): シグナルフレームから元のコンテキストを復元
    if syscall_num == crate::syscall::SyscallNumber::RtSigreturn as u64 {
        rt_sigreturn(kstack);
        return 0;
    }

    // シグナルを持つ current process を取得
    let pid = match current_pid() {
        Some(p) => p,
        None => return syscall_ret,
    };

    // 送達すべきシグナルを1つ取り出す
    let sig = match with_process_mut(pid, |p| p.signal_state_mut().take_next_deliverable()) {
        Some(Some(s)) => s,
        _ => return syscall_ret,
    };

    let action = with_process(pid, |p| p.signal_state().action(sig))
        .unwrap_or(SigAction::default_action());

    if action.is_ignored() {
        return syscall_ret;
    }

    if !action.has_user_handler() {
        // SIG_DFL
        match default_action(sig) {
            DefaultAction::Terminate => {
                crate::task::exit_current_task(sig as u64);
            }
            DefaultAction::Ignore => return syscall_ret,
        }
    }

    // ユーザーハンドラへリダイレクト
    unsafe { setup_signal_frame(kstack, sig, &action) };

    // ハンドラには syscall の戻り値ではなくシグナル番号が RDI に入る（フレーム設定済み）
    // RAX はハンドラには見えないが一応 0 にする
    0
}

/// int 0x80 割り込みフレームにシグナルフレームを積み、ハンドラへリダイレクトする
///
/// # Safety
/// `kstack` は有効な割り込みスタックフレームを指している必要がある。
unsafe fn setup_signal_frame(kstack: *mut u64, sig: usize, action: &SigAction) {
    // 割り込みフレームから user RIP / RSP / RFLAGS を取得
    let user_rip    = kstack.add(15).read();
    let user_rflags = kstack.add(17).read();
    let user_rsp    = kstack.add(18).read();

    // ユーザースタック上にシグナルフレームを構築
    // レイアウト（低アドレス → 高アドレス, 新 RSP は先頭）:
    //   [new_rsp + 0]:  sa_restorer  ← ハンドラの戻り先（return address）
    //   [new_rsp + 8]:  saved RIP
    //   [new_rsp + 16]: saved RSP
    //   [new_rsp + 24]: saved RFLAGS
    //
    // ハンドラ呼び出し規約: x86-64 では `call` の直後は RSP % 16 == 8 なので、
    // 戻り番地を積んだ直後のスタックトップとして new_rsp を渡す。
    const FRAME_BYTES: u64 = 32;
    let aligned = (user_rsp.wrapping_sub(FRAME_BYTES)) & !15u64;
    let new_rsp = aligned.wrapping_sub(8); // call 直後を模倣: RSP % 16 == 8

    // フレームをユーザースタックに書き込む
    let ok = write_signal_frame(new_rsp, action.restorer, user_rip, user_rsp, user_rflags);
    if !ok {
        // ユーザースタックが不正 → 強制終了
        crate::task::exit_current_task(11); // SIGSEGV
    }

    // ハンドラ実行中は action.mask のシグナルをブロック
    if let Some(pid) = current_pid() {
        with_process_mut(pid, |p| {
            p.signal_state_mut().mask |= action.mask;
        });
    }

    // 割り込みフレームを書き換えてハンドラへリダイレクト
    kstack.add(8).write(sig as u64);    // RDI = シグナル番号（ハンドラの第1引数）
    kstack.add(15).write(action.handler); // user RIP → ハンドラ
    kstack.add(18).write(new_rsp);        // user RSP → シグナルフレーム先頭
}

/// シグナルフレームをユーザースタックに書き込む
fn write_signal_frame(
    new_rsp: u64,
    restorer: u64,
    saved_rip: u64,
    saved_rsp: u64,
    saved_rflags: u64,
) -> bool {
    // 書き込みアドレスの検証（32バイト）
    if !crate::syscall::validate_user_ptr(new_rsp, 32) {
        return false;
    }
    crate::syscall::with_user_memory_access(|| unsafe {
        let p = new_rsp as *mut u64;
        p.add(0).write(restorer);      // return address
        p.add(1).write(saved_rip);
        p.add(2).write(saved_rsp);
        p.add(3).write(saved_rflags);
    });
    true
}

/// rt_sigreturn システムコール
///
/// シグナルハンドラから戻るときに呼ばれる。
/// シグナルフレームから保存された RIP / RSP / RFLAGS を復元する。
///
/// # 引数
/// - `kstack`: int 0x80 割り込みスタックフレーム先頭ポインタ
pub fn rt_sigreturn(kstack: *mut u64) {
    // ハンドラが `ret` した後、restorer が int 0x80 (RAX=15) を実行する。
    // `ret` で sa_restorer を pop したので、user RSP は +8 されている。
    // つまり user_rsp は saved_rip の直前を指している。
    let user_rsp = unsafe { kstack.add(18).read() };

    if !crate::syscall::validate_user_ptr(user_rsp, 24) {
        crate::task::exit_current_task(11); // SIGSEGV
    }

    let (saved_rip, saved_rsp, saved_rflags) =
        crate::syscall::with_user_memory_access(|| unsafe {
            let p = user_rsp as *const u64;
            (p.add(0).read(), p.add(1).read(), p.add(2).read())
        });

    unsafe {
        kstack.add(15).write(saved_rip);
        kstack.add(17).write(saved_rflags);
        kstack.add(18).write(saved_rsp);
    }

    // ハンドラ実行中にブロックしていたマスクを元に戻す（簡易: マスクをクリア）
    // TODO: シグナルフレームに旧マスクを保存して正確に復元する
    if let Some(pid) = current_pid() {
        with_process_mut(pid, |p| p.signal_state_mut().mask = 0);
    }
}

// ---- ヘルパー関数 -------------------------------------------------------

fn current_pid() -> Option<ProcessId> {
    let tid = current_thread_id()?;
    crate::task::with_thread(tid, |t| t.process_id())
}

/// 指定プロセスの最初のスレッドを起床させる
fn wake_first_thread_of(pid: ProcessId) {
    let mut tid_opt = None;
    crate::task::for_each_thread(|t| {
        if tid_opt.is_none() && t.process_id() == pid {
            tid_opt = Some(t.id());
        }
    });
    if let Some(tid) = tid_opt {
        crate::task::wake_thread(tid);
    }
}

/// プロセスを即座に強制終了する（SIGKILL 用）
fn kill_process_immediately(pid: ProcessId, exit_code: u64) {
    // 現在のプロセスなら exit_current_task で終了
    if let Some(cur_pid) = current_pid() {
        if cur_pid == pid {
            crate::task::exit_current_task(exit_code);
        }
    }
    // 他プロセスのスレッドをすべて Terminated にして Zombie に遷移させる
    let mut tids = alloc::vec::Vec::new();
    crate::task::for_each_thread(|t| {
        if t.process_id() == pid {
            tids.push(t.id());
        }
    });
    for tid in tids {
        crate::task::terminate_thread(tid);
    }
    crate::task::mark_process_exited(pid, exit_code);
    // 親に SIGCHLD を届ける
    deliver_sigchld_to_parent(pid);
}
