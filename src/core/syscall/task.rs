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

	let name_bytes = unsafe { core::slice::from_raw_parts(name_ptr as *const u8, name_len) };
	let name = match core::str::from_utf8(name_bytes) {
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
