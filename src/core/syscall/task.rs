/// タスク関連システムコール

pub fn yield_now() -> u64 {
    crate::task::yield_now();
    0
}

/// 現在のスレッドを終了
pub fn exit(_code: u64) -> u64 {
    if let Some(id) = crate::task::current_thread_id() {
        crate::task::terminate_thread(id);
        0
    } else {
        crate::syscall::EINVAL
    }
}

/// 現在のスレッドIDを取得
pub fn get_thread_id() -> u64 {
    match crate::task::current_thread_id() {
        Some(id) => id.as_u64(),
        None => crate::syscall::EINVAL,
    }
}

/// スレッドIDからプロセスの権限レベルを取得
///
/// # 引数
/// - `tid_val`: スレッドID (u64)
///
/// # 戻り値
/// 0=Core, 1=Service, 2=User, またはエラー (#22: ディスクサービスの特権検証に使用)
pub fn get_thread_privilege(tid_val: u64) -> u64 {
    // スレッドIDに対応するプロセスIDを探す
    let mut found_pid: Option<crate::task::ProcessId> = None;
    crate::task::for_each_thread(|t| {
        if found_pid.is_none() && t.id().as_u64() == tid_val {
            found_pid = Some(t.process_id());
        }
    });

    let pid = match found_pid {
        Some(p) => p,
        None => return crate::syscall::EINVAL,
    };

    match crate::task::with_process(pid, |p| p.privilege()) {
        Some(crate::task::PrivilegeLevel::Core) => 0,
        Some(crate::task::PrivilegeLevel::Service) => 1,
        Some(crate::task::PrivilegeLevel::User) => 2,
        None => crate::syscall::EINVAL,
    }
}

/// スレッド名からIDを取得
pub fn get_thread_id_by_name(name_ptr: u64, name_len: u64) -> u64 {
    const MAX_NAME_LEN: usize = 64;
    if name_ptr == 0 {
        return crate::syscall::EINVAL;
    }
    let name_len = name_len as usize;
    if name_len == 0 || name_len > MAX_NAME_LEN {
        return crate::syscall::EINVAL;
    }
    if !crate::syscall::validate_user_ptr(name_ptr, name_len as u64) {
        return crate::syscall::EFAULT;
    }

    let mut name_buf = [0u8; MAX_NAME_LEN];
    crate::syscall::with_user_memory_access(|| unsafe {
        let src = core::slice::from_raw_parts(name_ptr as *const u8, name_len);
        name_buf[..name_len].copy_from_slice(src);
    });
    let name = match core::str::from_utf8(&name_buf[..name_len]) {
        Ok(s) => s,
        Err(_) => return crate::syscall::EINVAL,
    };

    let mut found: Option<u64> = None;
    crate::task::for_each_thread(|t| {
        if found.is_none() && t.name() == name {
            found = Some(t.id().as_u64());
        }
    });

    match found {
        Some(id) => id,
        None => crate::syscall::ENOENT,
    }
}
