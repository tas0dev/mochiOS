use crate::elf::loader as elf_loader;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use core::convert::TryInto;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::structures::paging::Mapper;

/// `.service` 実行を許可するサービスマネージャープロセスID
/// 0 は未登録。
static SERVICE_MANAGER_PID: AtomicU64 = AtomicU64::new(0);
const EM_X86_64: u16 = 0x3E;
static EXEC_ASLR_COUNTER: AtomicU64 = AtomicU64::new(0);
const STACK_TOP_BASE: u64 = 0x0000_7FFF_FFF0_0000;
const STACK_ASLR_MAX_PAGES: u64 = 4096; // 16MiB
const USER_STACK_SIZE_PAGES: usize = 32; // 128KiB stack
const TLS_BASE_MIN: u64 = 0x3000_0000;
const TLS_ASLR_MAX_PAGES: u64 = 0x4000; // 64MiB
const INITIAL_TLS_SIZE: u64 = 4096;

struct InitialUserStack {
    stack_base_vaddr: u64,
    stack_end_vaddr: u64,
    initial_rsp: u64,
    page_data: Vec<u8>,
}

struct UserPageTableGuard(Option<u64>);

impl UserPageTableGuard {
    fn new(table_phys: u64) -> Self {
        Self(Some(table_phys))
    }

    fn disarm(&mut self) {
        self.0.take();
    }
}

impl Drop for UserPageTableGuard {
    fn drop(&mut self) {
        if let Some(table_phys) = self.0.take() {
            let _ = crate::mem::paging::destroy_user_page_table(table_phys);
        }
    }
}

/// サービスマネージャーPIDを登録する（IDベース認可）
pub fn register_service_manager_pid(pid: u64) {
    SERVICE_MANAGER_PID.store(pid, Ordering::SeqCst);
}

#[inline]
fn aslr_mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

fn next_aslr_seed(tag: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for b in tag.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    if EXEC_ASLR_COUNTER.load(Ordering::Relaxed) == 0 {
        let mut init = crate::cpu::boot_entropy_u64() ^ 0x7c4a_7f73_d3e1_9b1d;
        if init == 0 {
            init = 1;
        }
        let _ = EXEC_ASLR_COUNTER.compare_exchange(0, init, Ordering::SeqCst, Ordering::Relaxed);
    }
    let ctr = EXEC_ASLR_COUNTER.fetch_add(0x9e37_79b9_7f4a_7c15, Ordering::Relaxed);
    let ticks = crate::interrupt::timer::get_ticks();
    let tid = crate::task::current_thread_id()
        .map(|t| t.as_u64())
        .unwrap_or(0);
    let hw = crate::cpu::hw_random_u64().unwrap_or(0);
    let boot = crate::cpu::boot_entropy_u64();
    aslr_mix64(hash ^ ctr ^ ticks.rotate_left(17) ^ tid.rotate_left(7) ^ hw.rotate_left(29) ^ boot)
}

#[inline]
fn aslr_offset_pages(seed: u64, max_pages: u64) -> u64 {
    if max_pages == 0 {
        0
    } else {
        aslr_mix64(seed) % max_pages
    }
}

fn caller_can_launch_service() -> bool {
    let caller = crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()));
    let Some(caller_pid) = caller else {
        // カーネルコンテキストからの起動は許可
        return true;
    };

    if crate::task::with_process(caller_pid, |p| {
        p.privilege() == crate::task::PrivilegeLevel::Core
    })
    .unwrap_or(false)
    {
        return true;
    }

    let manager_pid_raw = SERVICE_MANAGER_PID.load(Ordering::SeqCst);
    if manager_pid_raw == 0 || caller_pid.as_u64() != manager_pid_raw {
        return false;
    }
    let manager_pid = crate::task::ProcessId::from_u64(manager_pid_raw);
    crate::task::with_process(manager_pid, |p| {
        let state = p.state();
        let alive = state != crate::task::ProcessState::Zombie
            && state != crate::task::ProcessState::Terminated;
        let privileged = matches!(
            p.privilege(),
            crate::task::PrivilegeLevel::Service | crate::task::PrivilegeLevel::Core
        );
        alive && privileged
    })
    .unwrap_or(false)
}

fn caller_is_service_or_core() -> bool {
    crate::task::current_thread_id()
        .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
        .and_then(|pid| crate::task::with_process(pid, |p| p.privilege()))
        .is_some_and(|lvl| {
            matches!(
                lvl,
                crate::task::PrivilegeLevel::Core | crate::task::PrivilegeLevel::Service
            )
        })
}

fn read_nul_args_from_user(
    args_ptr: u64,
    max_total_bytes: usize,
    max_args: usize,
) -> Result<Vec<String>, u64> {
    use crate::syscall::types::{EFAULT, EINVAL};

    if args_ptr == 0 {
        return Ok(Vec::new());
    }
    if !crate::syscall::validate_user_ptr(args_ptr, max_total_bytes as u64) {
        return Err(EFAULT);
    }

    let mut storage: Vec<u8> = Vec::new();
    crate::syscall::with_user_memory_access(|| unsafe {
        let ptr = args_ptr as *const u8;
        for i in 0..max_total_bytes {
            let b = ptr.add(i).read_volatile();
            storage.push(b);
            let len = storage.len();
            if len >= 2 && storage[len - 1] == 0 && storage[len - 2] == 0 {
                break;
            }
        }
    });

    let mut out = Vec::new();
    for s in storage.split(|&b| b == 0) {
        if s.is_empty() {
            continue;
        }
        let text = core::str::from_utf8(s).map_err(|_| EINVAL)?;
        out.push(String::from(text));
        if out.len() >= max_args {
            break;
        }
    }
    Ok(out)
}

/// カーネル内から実行可能ファイルを読み込み実行するシステムコール
/// args_ptr: ヌル区切り引数文字列へのポインタ（"arg1\0arg2\0\0"形式）、0 なら引数なし
pub fn exec_kernel(path_ptr: u64, args_ptr: u64) -> u64 {
    let mut provided_path: Option<String> = None;
    if path_ptr != 0 {
        let path = match crate::syscall::read_user_cstring(path_ptr, 256) {
            Ok(s) => s,
            Err(_) => return crate::syscall::types::EINVAL,
        };
        provided_path = Some(path);
    }
    let path = provided_path.as_deref().unwrap_or("/hello.bin");

    // ユーザー空間からはサービス（.serviceで終わる名前）を起動できない
    if path.ends_with(".service") && !caller_can_launch_service() {
        return crate::syscall::types::EPERM;
    }

    let extra_args_owned = match read_nul_args_from_user(args_ptr, 512, 64) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let extra_args: Vec<&str> = extra_args_owned.iter().map(|s| s.as_str()).collect();
    exec_internal(path, None, &extra_args)
}

/// 名前を指定してカーネル内から実行可能ファイルを実行する（カーネル内部用）
pub fn exec_kernel_with_name(path: &str, name: &str) -> u64 {
    exec_internal(path, Some(name), &[])
}

fn exec_internal(path: &str, name_override: Option<&str>, args: &[&str]) -> u64 {
    let process_name = name_override.unwrap_or(path);
    if let Some(data) = crate::init::fs::read(path) {
        exec_with_data(&data, process_name, path, args, None)
    } else if let Some(data) = crate::kmod::fs::read_all(path) {
        exec_with_data(&data, process_name, path, args, None)
    } else {
        crate::warn!("exec: file not found: {}", path);
        crate::syscall::types::ENOENT
    }
}

/// Exec by streaming image with zero-copy frame transfer when possible.
pub fn exec_from_fs_stream(path_ptr: u64, args_ptr: u64) -> u64 {
    use crate::mem::frame;
    use crate::mem::paging;
    use x86_64::PhysAddr;

    let path = match crate::syscall::read_user_cstring(path_ptr, 256) {
        Ok(s) => s,
        Err(_) => return crate::syscall::types::EINVAL,
    };

    if path.ends_with(".service") && !caller_can_launch_service() {
        return crate::syscall::types::EPERM;
    }

    let extra_args_owned = match read_nul_args_from_user(args_ptr, 512, 64) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let extra_args: Vec<&str> = extra_args_owned.iter().map(|s| s.as_str()).collect();

    let data = match crate::kmod::fs::read_all(&path) {
        Some(d) => d,
        None => return crate::syscall::types::ENOENT,
    };
    return exec_with_data(&data, &path, &path, &extra_args, None);

    // 互換経路（現在の構成では未使用）
    let fs_tid = match crate::syscall::fs::fs_service_tid() {
        Some(t) => t,
        None => {
            let data = match crate::kmod::fs::read_all(&path) {
                Some(d) => d,
                None => return crate::syscall::types::ENOENT,
            };
            return exec_with_data(&data, &path, &path, &extra_args, None);
        }
    };

    let req = crate::syscall::fs::FsRequest {
        op: crate::syscall::fs::FsRequest::OP_EXEC_STREAM,
        arg1: 1, // mapped-write mode
        arg2: 0,
        path: match crate::syscall::fs::encode_fs_path(&path) {
            Ok(p) => p,
            Err(e) => return e,
        },
    };

    let header = match crate::syscall::fs::fs_service_request(fs_tid, &req) {
        Ok(h) => h,
        Err(e) => return e,
    };
    if header.status < 0 {
        return (-header.status) as u64;
    }
    let total = header.len as usize;
    if total == 0 {
        return crate::syscall::types::EINVAL;
    }

    // Phase 2: allocate frames
    let pages = (total + 4095) / 4096;
    let mut frames: Vec<u64> = Vec::new();
    // extra_frames collects frames allocated during mapping (e.g., for BSS) that are not part of initial frames
    let mut extra_frames: Vec<u64> = Vec::new();
    for _ in 0..pages {
        match frame::allocate_frame() {
            Ok(f) => frames.push(f.start_address().as_u64()),
            Err(_) => {
                // deallocate any allocated initial frames
                for p in frames.iter() {
                    let fr = PhysAddr::new(*p);
                    let framef = x86_64::structures::paging::PhysFrame::containing_address(fr);
                    let _ = frame::deallocate_frame(framef);
                }
                // deallocate extra frames if any
                for p in extra_frames.iter() {
                    let fr = PhysAddr::new(*p);
                    let framef = x86_64::structures::paging::PhysFrame::containing_address(fr);
                    let _ = frame::deallocate_frame(framef);
                }
                return crate::syscall::types::ENOMEM;
            }
        }
    }

    // Phase 3: map frames into legacy service address space
    let fs_pid = match crate::task::find_process_id_by_name("fs_legacy.service") {
        Some(p) => p,
        None => return crate::syscall::types::ESRCH,
    };

    let map_start_res = crate::task::with_process_mut(fs_pid, |proc| {
        if proc.heap_start() == 0 {
            let default_base = 0x5000_0000u64;
            proc.set_heap_start(default_base);
            proc.set_heap_end(default_base);
        }
        let base = proc.heap_end();
        let map_start = base.checked_add(0xfff).map(|v| v & !0xfffu64).unwrap_or(0);
        if map_start == 0 || map_start > 0x0000_7FFF_FFFF_FFFF {
            return Err(crate::syscall::types::ENOMEM);
        }
        let pt_phys = match proc.page_table() {
            Some(p) => p,
            None => return Err(crate::syscall::types::ENOMEM),
        };
        let mut mapped_pages = 0usize;
        for i in 0..frames.len() {
            let va = map_start + (i as u64) * 4096u64;
            if let Err(_) =
                crate::mem::paging::map_physical_range_to_user(pt_phys, va, frames[i], 4096)
            {
                for mapped in 0..mapped_pages {
                    let mapped_va = map_start + (mapped as u64) * 4096u64;
                    let _ = crate::mem::paging::unmap_page_in_table(pt_phys, mapped_va);
                }
                return Err(crate::syscall::types::ENOMEM);
            }
            mapped_pages += 1;
        }
        let map_bytes = (frames.len() as u64)
            .checked_mul(4096)
            .ok_or(crate::syscall::types::ENOMEM)?;
        let new_end = match map_start.checked_add(map_bytes) {
            Some(v) => v,
            None => {
                for mapped in 0..mapped_pages {
                    let mapped_va = map_start + (mapped as u64) * 4096u64;
                    let _ = crate::mem::paging::unmap_page_in_table(pt_phys, mapped_va);
                }
                return Err(crate::syscall::types::ENOMEM);
            }
        };
        proc.set_heap_end(new_end);
        Ok(map_start)
    });

    let map_start = match map_start_res {
        Some(Ok(v)) => v,
        Some(Err(e)) => {
            for p in frames.iter() {
                let fr = PhysAddr::new(*p);
                let framef = x86_64::structures::paging::PhysFrame::containing_address(fr);
                let _ = frame::deallocate_frame(framef);
            }
            return e;
        }
        None => {
            for p in frames.iter() {
                let fr = PhysAddr::new(*p);
                let framef = x86_64::structures::paging::PhysFrame::containing_address(fr);
                let _ = frame::deallocate_frame(framef);
            }
            return crate::syscall::types::ESRCH;
        }
    };

    // Phase 4: notify legacy stream backend of mapping
    // For large transfers, prefer sending only a map header so the page list is not sent over IPC.
    if !crate::syscall::ipc::send_map_header_from_kernel(fs_tid, map_start, total as u64) {
        if let Some(fs_table) = crate::task::with_process(fs_pid, |p| p.page_table()).flatten() {
            let _ = crate::mem::paging::unmap_range_in_table(
                fs_table,
                map_start,
                (frames.len() as u64) * 4096,
            );
        }
        for p in frames.iter() {
            let fr = PhysAddr::new(*p);
            let framef = x86_64::structures::paging::PhysFrame::containing_address(fr);
            let _ = frame::deallocate_frame(framef);
        }
        // free extra_frames as well
        for p in extra_frames.iter() {
            let fr = PhysAddr::new(*p);
            let framef = x86_64::structures::paging::PhysFrame::containing_address(fr);
            let _ = frame::deallocate_frame(framef);
        }
        return crate::syscall::types::EIO;
    }

    // Phase 5: wait for legacy stream backend ack
    // Note: send_map_header_from_kernel sends only map_start+total; fs should write into mapped range and reply.
    let mut ack = [0u8; 8];
    let ack_wait_result = {
        let start_tick = crate::syscall::time::get_ticks();
        loop {
            if !crate::task::thread_id_exists(fs_tid) {
                break Err(crate::syscall::types::EIO);
            }
            match crate::syscall::ipc::recv_from_sender_for_kernel_nonblocking(fs_tid, &mut ack) {
                Ok(Some(_)) => break Ok(()),
                Ok(None) => {
                    if crate::syscall::time::get_ticks().saturating_sub(start_tick) > 500 {
                        break Err(crate::syscall::types::EIO);
                    }
                    crate::task::yield_now();
                }
                Err(_) => break Err(crate::syscall::types::EIO),
            }
        }
    };

    if ack_wait_result.is_err() {
        if let Some(fs_table) = crate::task::with_process(fs_pid, |p| p.page_table()).flatten() {
            let _ = crate::mem::paging::unmap_range_in_table(
                fs_table,
                map_start,
                (frames.len() as u64) * 4096,
            );
        }
        for p in frames.iter() {
            let fr = PhysAddr::new(*p);
            let framef = x86_64::structures::paging::PhysFrame::containing_address(fr);
            let _ = frame::deallocate_frame(framef);
        }
        for p in extra_frames.iter() {
            let fr = PhysAddr::new(*p);
            let framef = x86_64::structures::paging::PhysFrame::containing_address(fr);
            let _ = frame::deallocate_frame(framef);
        }
        return crate::syscall::types::EIO;
    }

    // Phase 6: try zero-copy mapping into new process
    let phys_off = match crate::mem::paging::physical_memory_offset() {
        Some(v) => v,
        None => return crate::syscall::types::EINVAL,
    };
    // Efficiently copy from consecutive physical frames into a buffer using page-wise memcpy.
    fn copy_frames_to_buf(frames: &Vec<u64>, phys_off: u64, dst: &mut [u8]) {
        let mut written = 0usize;
        let total_dst = dst.len();
        while written < total_dst {
            let frame_idx = written / 4096;
            let in_frame_off = written % 4096;
            let phys = frames[frame_idx] + in_frame_off as u64;
            let avail_in_frame = core::cmp::min(4096 - in_frame_off, total_dst - written);
            unsafe {
                let src = (phys + phys_off) as *const u8;
                let dst_ptr = dst.as_mut_ptr().add(written);
                core::ptr::copy_nonoverlapping(src, dst_ptr, avail_in_frame);
            }
            written += avail_in_frame;
        }
    }

    // read initial header chunk
    let mut header_read = core::cmp::min(total, 65536);
    if header_read < 64 {
        header_read = core::cmp::min(total, 4096);
    }
    let mut header_buf: Vec<u8> = vec![0u8; header_read];
    copy_frames_to_buf(&frames, phys_off, &mut header_buf);

    let eh_opt = crate::elf::loader::parse_elf_header(&header_buf);
    if eh_opt.is_none() {
        // fallback to copy path
        crate::warn!("exec: ELF header not parsable for zero-copy, falling back to copy path");
        let mut image: Vec<u8> = vec![0u8; total];
        copy_frames_to_buf(&frames, phys_off, &mut image);
        let _ = crate::mem::paging::unmap_range_in_table(
            crate::task::with_process(fs_pid, |p| p.page_table())
                .flatten()
                .unwrap_or(0),
            map_start,
            (frames.len() as u64) * 4096,
        );
        for p in frames.iter() {
            let fr = PhysAddr::new(*p);
            let framef = x86_64::structures::paging::PhysFrame::containing_address(fr);
            let _ = frame::deallocate_frame(framef);
        }
        return exec_with_data(
            &image,
            path.as_str(),
            &path,
            &extra_args,
            delegated_parent_pid(),
        );
    }
    let eh = eh_opt.unwrap();
    let phoff = eh.e_phoff as usize;
    let phentsz = eh.e_phentsize as usize;
    let phnum = eh.e_phnum as usize;

    if phoff
        .checked_add(phentsz.checked_mul(phnum).unwrap_or(0))
        .unwrap_or(usize::MAX)
        > total
    {
        crate::warn!("exec: ELF phdrs exceed total size");
        let mut image: Vec<u8> = vec![0u8; total];
        copy_frames_to_buf(&frames, phys_off, &mut image);
        let _ = crate::mem::paging::unmap_range_in_table(
            crate::task::with_process(fs_pid, |p| p.page_table())
                .flatten()
                .unwrap_or(0),
            map_start,
            (frames.len() as u64) * 4096,
        );
        for p in frames.iter() {
            let fr = PhysAddr::new(*p);
            let framef = x86_64::structures::paging::PhysFrame::containing_address(fr);
            let _ = frame::deallocate_frame(framef);
        }
        return exec_with_data(
            &image,
            path.as_str(),
            &path,
            &extra_args,
            delegated_parent_pid(),
        );
    }

    // alignment check
    let mut misaligned = false;
    for i in 0..phnum {
        let off_hdr = phoff + i * phentsz;
        if let Some(ph) = crate::elf::loader::parse_phdr(&header_buf, off_hdr) {
            if ph.p_type == crate::elf::loader::PT_LOAD {
                if (ph.p_vaddr & 0xfff) != (ph.p_offset & 0xfff) {
                    misaligned = true;
                    break;
                }
            }
        }
    }

    if misaligned {
        crate::warn!("exec: ELF segments misaligned for zero-copy, falling back");
        let mut image: Vec<u8> = vec![0u8; total];
        copy_frames_to_buf(&frames, phys_off, &mut image);
        let _ = crate::mem::paging::unmap_range_in_table(
            crate::task::with_process(fs_pid, |p| p.page_table())
                .flatten()
                .unwrap_or(0),
            map_start,
            (frames.len() as u64) * 4096,
        );
        for p in frames.iter() {
            let fr = PhysAddr::new(*p);
            let framef = x86_64::structures::paging::PhysFrame::containing_address(fr);
            let _ = frame::deallocate_frame(framef);
        }
        return exec_with_data(
            &image,
            path.as_str(),
            &path,
            &extra_args,
            delegated_parent_pid(),
        );
    }

    // create new page table for target process
    let new_pt_phys = match crate::mem::paging::create_user_page_table() {
        Ok(p) => p,
        Err(_) => {
            crate::warn!("exec: failed to create user page table for zero-copy, falling back");
            let mut image: Vec<u8> = vec![0u8; total];
            copy_frames_to_buf(&frames, phys_off, &mut image);
            let _ = crate::mem::paging::unmap_range_in_table(
                crate::task::with_process(fs_pid, |p| p.page_table())
                    .flatten()
                    .unwrap_or(0),
                map_start,
                (frames.len() as u64) * 4096,
            );
            for p in frames.iter() {
                let fr = PhysAddr::new(*p);
                let framef = x86_64::structures::paging::PhysFrame::containing_address(fr);
                let _ = frame::deallocate_frame(framef);
            }
            return exec_with_data(
                &image,
                path.as_str(),
                &path,
                &extra_args,
                delegated_parent_pid(),
            );
        }
    };
    let mut new_pt_guard = UserPageTableGuard::new(new_pt_phys);

    // map frames into new page table at segment vaddrs
    for i in 0..phnum {
        let off_hdr = phoff + i * phentsz;
        if let Some(ph) = crate::elf::loader::parse_phdr(&header_buf, off_hdr) {
            if ph.p_type != crate::elf::loader::PT_LOAD {
                continue;
            }
            let vaddr = ph.p_vaddr;
            let filesz = ph.p_filesz as usize;
            let memsz = ph.p_memsz as usize;
            let src_off = ph.p_offset as usize;

            let file_end = src_off + filesz;
            let start_page = (vaddr & !0xfff_u64) as u64;
            let end_page = ((vaddr + (memsz as u64) + 0xfff) & !0xfff_u64) as u64;
            let mut page = start_page;
            while page < end_page {
                let page_index = ((page as i128 - vaddr as i128) / 4096) as isize;
                let file_page_off = (src_off as isize + page_index as isize * 4096) as isize;
                let phys_frame = if file_page_off >= 0 && (file_page_off as usize) < total {
                    let idx = (file_page_off as usize) / 4096;
                    if idx >= frames.len() {
                        misaligned = true;
                        break;
                    }
                    frames[idx]
                } else {
                    // bss -> allocate and zero
                    match frame::allocate_frame() {
                        Ok(f) => {
                            let p = f.start_address().as_u64();
                            // record extra frame so it can be released on failure
                            extra_frames.push(p);
                            unsafe { core::ptr::write_bytes((p + phys_off) as *mut u8, 0, 4096) };
                            p
                        }
                        Err(_) => {
                            misaligned = true;
                            break;
                        }
                    }
                };

                if let Err(_) = crate::mem::paging::map_physical_range_to_user(
                    new_pt_phys,
                    page,
                    phys_frame,
                    4096,
                ) {
                    crate::warn!(
                        "exec: failed to map phys {:#x} to vaddr {:#x}",
                        phys_frame,
                        page
                    );
                    misaligned = true;
                    break;
                }

                // adjust flags for executability/writability
                if (ph.p_flags & 0x1) != 0 {
                    // executable: clear NX on this page
                    let phys_off_local = match crate::mem::paging::physical_memory_offset() {
                        Some(v) => v,
                        None => {
                            misaligned = true;
                            0
                        }
                    };
                    let l4 = unsafe {
                        &mut *(((new_pt_phys) + phys_off_local)
                            as *mut x86_64::structures::paging::PageTable)
                    };
                    let mut pt = unsafe {
                        x86_64::structures::paging::OffsetPageTable::new(
                            l4,
                            x86_64::VirtAddr::new(phys_off_local),
                        )
                    };
                    let pg = x86_64::structures::paging::Page::<x86_64::structures::paging::Size4KiB>::containing_address(x86_64::VirtAddr::new(page));
                    let mut flags = x86_64::structures::paging::PageTableFlags::PRESENT
                        | x86_64::structures::paging::PageTableFlags::USER_ACCESSIBLE;
                    if (ph.p_flags & 0x2) != 0 {
                        flags |= x86_64::structures::paging::PageTableFlags::WRITABLE;
                    }
                    unsafe {
                        let _ = pt.update_flags(pg, flags).map(|f| f.flush());
                    }
                }

                page = match page.checked_add(4096) {
                    Some(v) => v,
                    None => break,
                };
            }
            if misaligned {
                break;
            }
        }
    }

    if misaligned {
        // fallback
        crate::warn!("exec: zero-copy mapping failed during mapping loop, falling back");
        let mut image: Vec<u8> = vec![0u8; total];
        copy_frames_to_buf(&frames, phys_off, &mut image);
        let _ = crate::mem::paging::unmap_range_in_table(
            crate::task::with_process(fs_pid, |p| p.page_table())
                .flatten()
                .unwrap_or(0),
            map_start,
            (frames.len() as u64) * 4096,
        );
        for p in frames.iter() {
            let fr = PhysAddr::new(*p);
            let framef = x86_64::structures::paging::PhysFrame::containing_address(fr);
            let _ = frame::deallocate_frame(framef);
        }
        return exec_with_data(
            &image,
            path.as_str(),
            &path,
            &extra_args,
            delegated_parent_pid(),
        );
    }

    // unmap from fs but preserve frames (ownership transferred)
    let fs_table_phys = crate::task::with_process(fs_pid, |p| p.page_table())
        .flatten()
        .unwrap_or(0);
    let _ = crate::mem::paging::unmap_range_in_table_preserve_frames(
        fs_table_phys,
        map_start,
        (frames.len() as u64) * 4096,
    );

    // Finalize exec: compute phdr_vaddr and build stack/tls, create process/thread
    let mut load_base: u64 = 0;
    let mut load_base_set = false;
    for i in 0..phnum {
        let off_hdr = phoff + i * phentsz;
        if let Some(ph) = crate::elf::loader::parse_phdr(&header_buf, off_hdr) {
            if ph.p_type == crate::elf::loader::PT_LOAD {
                if !load_base_set {
                    load_base = ph.p_vaddr.saturating_sub(ph.p_offset);
                    load_base_set = true;
                }
            }
        }
    }
    let phdr_vaddr = load_base.saturating_add(eh.e_phoff);
    // reuse many steps from exec_with_data for stack/tls and process creation
    // Build initial stack
    let argv0 = path.as_str().rsplit('/').next().unwrap_or(path.as_str());
    let mut all_args: Vec<&str> = Vec::new();
    all_args.push(argv0);
    for a in &extra_args {
        all_args.push(a);
    }
    let envs: [&str; 0] = [];
    let auxv_entries = [
        (3u64, phdr_vaddr),
        (4u64, eh.e_phentsize as u64),
        (5u64, eh.e_phnum as u64),
        (6u64, 4096u64),
        (7u64, 0u64),
        (8u64, 0u64),
        (9u64, eh.e_entry),
        (11u64, 0u64),
        (12u64, 0u64),
        (13u64, 0u64),
        (14u64, 0u64),
        (16u64, 0u64),
        (17u64, 100u64),
        (23u64, 0u64),
        (25u64, 0u64),
        (31u64, 0u64),
        (0u64, 0u64),
    ];
    let InitialUserStack {
        stack_base_vaddr,
        stack_end_vaddr,
        initial_rsp,
        page_data,
    } = match build_initial_user_stack(
        next_aslr_seed(path.as_str()),
        &all_args,
        &envs,
        path.as_str(),
        &auxv_entries,
    ) {
        Ok(s) => s,
        Err(e) => return e,
    };

    // map stack and heap/tls
    if crate::mem::paging::map_and_copy_segment_to(
        new_pt_phys,
        stack_base_vaddr,
        0,
        (USER_STACK_SIZE_PAGES - 1) as u64 * 4096,
        &[],
        true,
        false,
    )
    .is_err()
    {
        return crate::syscall::types::EINVAL;
    }
    let top_page_vaddr = stack_end_vaddr - 4096;
    if crate::mem::paging::map_and_copy_segment_to(
        new_pt_phys,
        top_page_vaddr,
        4096,
        4096,
        &page_data,
        true,
        false,
    )
    .is_err()
    {
        return crate::syscall::types::EINVAL;
    }
    const HEAP_BASE_MIN: u64 = 0x4000_0000;
    const HEAP_ASLR_MAX_PAGES: u64 = 0x8000;
    let default_heap_base = HEAP_BASE_MIN.saturating_add(
        aslr_offset_pages(
            next_aslr_seed(path.as_str()) ^ 0x4a11_6b5c,
            HEAP_ASLR_MAX_PAGES,
        ) * 4096,
    );
    let heap_map_size: u64 = 4096 * 2;
    if crate::mem::paging::map_and_copy_segment_to(
        new_pt_phys,
        default_heap_base,
        0,
        heap_map_size,
        &[],
        true,
        false,
    )
    .is_err()
    {
        return crate::syscall::types::EINVAL;
    }
    let initial_fs_base = match map_initial_tls(new_pt_phys, next_aslr_seed(path.as_str())) {
        Ok(b) => b,
        Err(e) => return e,
    };

    // create process and thread
    let parent_pid = delegated_parent_pid();
    let privilege = resolve_exec_privilege(path.as_str(), &path);
    let mut proc = crate::task::Process::new(path.as_str(), privilege, parent_pid, 0);
    let proc_pid = proc.id();
    proc.set_page_table(new_pt_phys);
    proc.set_stack_bottom(stack_base_vaddr);
    proc.set_stack_top(stack_end_vaddr);
    if crate::task::add_process(proc).is_none() {
        let _ = crate::mem::paging::destroy_user_page_table(new_pt_phys);
        return crate::syscall::types::EINVAL;
    }
    new_pt_guard.disarm();

    const KERNEL_THREAD_STACK_SIZE: usize = 4096 * 32; // 128KB
    let kstack = match crate::task::thread::allocate_kernel_stack(KERNEL_THREAD_STACK_SIZE) {
        Some(a) => a,
        None => {
            let _ = crate::task::remove_process(proc_pid);
            return crate::syscall::types::ENOMEM;
        }
    };
    let mut thread = crate::task::Thread::new_usermode(
        proc_pid,
        path.as_str(),
        eh.e_entry,
        initial_rsp,
        kstack,
        KERNEL_THREAD_STACK_SIZE,
    );
    if crate::task::add_thread(thread).is_none() {
        let _ = crate::task::remove_process(proc_pid);
        let _ = crate::mem::paging::destroy_user_page_table(new_pt_phys);
        return crate::syscall::types::EINVAL;
    }

    crate::syscall::types::SUCCESS
}

#[inline]
fn resolve_exec_privilege(process_name: &str, exec_path: &str) -> crate::task::PrivilegeLevel {
    // .service は従来通り Service 権限で実行。
    // Binaries/drivers 配下は Service/Core 呼び出し元からの起動時に Service 権限を付与する。
    // Kagami / ViewKit / Binder はデスクトップ描画のため Service 権限を付与する。
    let is_driver_path =
        exec_path.starts_with("Binaries/drivers/") || exec_path.starts_with("/Binaries/drivers/");
    let is_kagami_viewkit_path = matches!(
        exec_path,
        "/Applications/Kagami.app/entry.elf"
            | "/Applications/ViewKit.app/entry.elf"
            | "/Applications/Binder.app/entry.elf"
            | "Applications/Kagami.app/entry.elf"
            | "Applications/ViewKit.app/entry.elf"
            | "Applications/Binder.app/entry.elf"
    );
    if process_name.ends_with(".service")
        || is_kagami_viewkit_path
        || (is_driver_path && caller_is_service_or_core())
    {
        crate::task::PrivilegeLevel::Service
    } else {
        crate::task::PrivilegeLevel::User
    }
}

fn map_initial_tls(table_phys: u64, aslr_seed: u64) -> Result<u64, u64> {
    let tls_base = TLS_BASE_MIN
        .saturating_add(aslr_offset_pages(aslr_seed ^ 0x19d7_3c6a, TLS_ASLR_MAX_PAGES) * 4096);
    let mut tls_data = vec![0u8; INITIAL_TLS_SIZE as usize];
    tls_data[..8].copy_from_slice(&tls_base.to_ne_bytes());
    match crate::mem::paging::map_and_copy_segment_to(
        table_phys,
        tls_base,
        INITIAL_TLS_SIZE,
        INITIAL_TLS_SIZE,
        &tls_data,
        true,
        false,
    ) {
        Ok(()) => Ok(tls_base),
        Err(e) => {
            crate::warn!(
                "Failed to map initial TLS block at {:#x}: {:?}",
                tls_base,
                e
            );
            Err(crate::syscall::types::EINVAL)
        }
    }
}

#[inline(never)]
fn build_initial_user_stack(
    aslr_seed: u64,
    argv: &[&str],
    envp: &[&str],
    execfn: &str,
    auxv_entries: &[(u64, u64)],
) -> Result<InitialUserStack, u64> {
    let stack_end_vaddr = STACK_TOP_BASE
        .saturating_sub(aslr_offset_pages(aslr_seed ^ 0x53a9_1e2d, STACK_ASLR_MAX_PAGES) * 4096);
    let stack_base_vaddr = stack_end_vaddr - (USER_STACK_SIZE_PAGES as u64 * 4096);

    let mut string_block = Vec::new();
    let mut argv_offsets = Vec::new();
    for arg in argv {
        argv_offsets.push(string_block.len());
        string_block.extend_from_slice(arg.as_bytes());
        string_block.push(0);
    }

    let mut envp_offsets = Vec::new();
    for env in envp {
        envp_offsets.push(string_block.len());
        string_block.extend_from_slice(env.as_bytes());
        string_block.push(0);
    }

    let random_offset = string_block.len();
    let mut rng = aslr_seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    for _ in 0..16 {
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        string_block.push((rng >> 33) as u8);
    }

    let execfn_offset = string_block.len();
    string_block.extend_from_slice(execfn.as_bytes());
    string_block.push(0);

    let string_area_len = string_block.len();
    let pointers_bytes =
        8 + (argv.len() * 8) + 8 + (envp.len() * 8) + 8 + (auxv_entries.len() * 16);
    let total_data_needed = string_area_len + pointers_bytes;
    let padding_len = (16 - (total_data_needed % 16)) % 16;
    let total_size = total_data_needed + padding_len;

    if total_size > 4096 {
        crate::warn!("Arguments too large for single page stack setup");
        return Err(crate::syscall::types::EINVAL);
    }

    let string_area_base = stack_end_vaddr - string_area_len as u64;
    let random_addr = string_area_base + random_offset as u64;
    let execfn_addr = string_area_base + execfn_offset as u64;
    let initial_rsp = stack_end_vaddr - total_size as u64;

    let mut page_data = Vec::new();
    let page_offset = total_size % 4096;
    let unused_space = if page_offset == 0 {
        0
    } else {
        4096 - page_offset
    };
    page_data.resize(unused_space, 0);

    page_data.extend_from_slice(&(argv.len() as u64).to_ne_bytes());
    for off in argv_offsets {
        let ptr = string_area_base + off as u64;
        page_data.extend_from_slice(&ptr.to_ne_bytes());
    }
    page_data.extend_from_slice(&0u64.to_ne_bytes());

    for off in envp_offsets {
        let ptr = string_area_base + off as u64;
        page_data.extend_from_slice(&ptr.to_ne_bytes());
    }
    page_data.extend_from_slice(&0u64.to_ne_bytes());

    for (key, value) in auxv_entries {
        let resolved_value = match *key {
            25 => random_addr, // AT_RANDOM
            31 => execfn_addr, // AT_EXECFN
            _ => *value,
        };
        page_data.extend_from_slice(&key.to_ne_bytes());
        page_data.extend_from_slice(&resolved_value.to_ne_bytes());
    }

    page_data.resize(page_data.len() + padding_len, 0);
    page_data.extend_from_slice(&string_block);

    if page_data.len() != 4096 {
        crate::warn!("internal: page_data.len() != 4096: {}", page_data.len());
        return Err(crate::syscall::types::EINVAL);
    }

    Ok(InitialUserStack {
        stack_base_vaddr,
        stack_end_vaddr,
        initial_rsp,
        page_data,
    })
}

/// メモリ上の ELF バッファからプロセスを生成する（内部共通実装）
fn delegated_parent_pid() -> Option<crate::task::ProcessId> {
    None
}

fn exec_with_data(
    data: &[u8],
    process_name: &str,
    exec_path: &str,
    args: &[&str],
    parent_override: Option<crate::task::ProcessId>,
) -> u64 {
    crate::debug!("exec: name={}", process_name);
    let aslr_seed = next_aslr_seed(process_name);

    {
        let data: &[u8] = data;
        // MED-27修正: エントリポイントが0の場合はELFが無効として拒否する
        // 以前はentry=0のままプロセスを作成し、仮想アドレス0にジャンプしていた
        let mut entry = match elf_loader::entry_point(data) {
            Some(e) if e != 0 => e,
            _ => {
                crate::warn!("exec: ELF entry point is 0 or missing, rejecting");
                return crate::syscall::types::EINVAL;
            }
        };
        crate::debug!("ELF entry: {:#x}", entry);
        let new_pt_phys = match crate::mem::paging::create_user_page_table() {
            Ok(phys) => phys,
            Err(e) => {
                crate::warn!(
                    "Failed to create user page table for {}: {:?}",
                    process_name,
                    e
                );
                return crate::syscall::types::EINVAL;
            }
        };
        let mut new_pt_guard = UserPageTableGuard::new(new_pt_phys);
        crate::debug!("Created user page table at {:#x}", new_pt_phys);

        // ELFアーキテクチャ検証 (MED-07)
        let mut phdr_vaddr: u64 = 0;
        let mut phentsize: u64 = 0;
        let mut phnum: u64 = 0;
        if let Some(eh) = elf_loader::parse_elf_header(data) {
            if eh.e_machine != EM_X86_64 {
                crate::warn!("ELF e_machine {:#x} is not x86-64, rejecting", eh.e_machine);
                return crate::syscall::types::EINVAL;
            }
            phentsize = eh.e_phentsize as u64;
            phnum = eh.e_phnum as u64;
            let phoff = eh.e_phoff as usize;
            let phentsz = eh.e_phentsize as usize;
            // phentszが0の場合は無限ループを防ぐため拒否 (MED-08)
            if phentsz == 0 {
                crate::warn!("ELF phentsize is 0, rejecting");
                return crate::syscall::types::EINVAL;
            }
            let phnum = eh.e_phnum as usize;
            let mut load_base: u64 = 0;
            let mut load_base_set = false;
            for i in 0..phnum {
                // オーバーフロー安全な乗算と加算 (MED-08)
                let off_hdr = match i.checked_mul(phentsz).and_then(|x| phoff.checked_add(x)) {
                    Some(o) if o < data.len() => o,
                    _ => {
                        crate::warn!("ELF program header offset overflow or out of bounds");
                        return crate::syscall::types::EINVAL;
                    }
                };
                if let Some(ph) = elf_loader::parse_phdr(data, off_hdr) {
                    if ph.p_type == elf_loader::PT_LOAD {
                        let vaddr = ph.p_vaddr;
                        let memsz = ph.p_memsz;
                        let filesz = ph.p_filesz;
                        let src_off = ph.p_offset as usize;
                        let flags = ph.p_flags;
                        let writable = (flags & 0x2) != 0;
                        let executable = (flags & 0x1) != 0;

                        // ELFセグメントのvaddrがユーザー空間内であることを検証 (CRIT-05)
                        const USER_SPACE_END: u64 = 0x0000_7FFF_FFFF_FFFF;
                        if vaddr >= USER_SPACE_END {
                            crate::warn!("ELF segment vaddr {:#x} is in kernel space", vaddr);
                            return crate::syscall::types::EINVAL;
                        }
                        if memsz > 0 {
                            match vaddr.checked_add(memsz) {
                                Some(e) if e <= USER_SPACE_END => {}
                                _ => {
                                    crate::warn!("ELF segment vaddr+memsz overflows user space");
                                    return crate::syscall::types::EINVAL;
                                }
                            }
                        }

                        // ELFセグメントの境界チェック (CRIT-04)
                        let src_end = match src_off.checked_add(filesz as usize) {
                            Some(e) if e <= data.len() => e,
                            _ => {
                                crate::warn!("ELF segment src offset+filesz out of bounds");
                                return crate::syscall::types::EINVAL;
                            }
                        };

                        crate::debug!(
                            "Mapping seg {} -> {:#x} (filesz={}, memsz={})",
                            i,
                            vaddr,
                            filesz,
                            memsz
                        );
                        let seg_src = &data[src_off..src_end];

                        if !load_base_set {
                            load_base = ph.p_vaddr.saturating_sub(ph.p_offset);
                            load_base_set = true;
                        }

                        if let Err(e) = crate::mem::paging::map_and_copy_segment_to(
                            new_pt_phys,
                            vaddr,
                            filesz,
                            memsz,
                            seg_src,
                            writable,
                            executable,
                        ) {
                            crate::warn!("Failed to map segment: {:?}", e);
                            return crate::syscall::types::EINVAL;
                        }
                    }
                }
            }

            phdr_vaddr = load_base.saturating_add(eh.e_phoff);
        }

        // __sinit は newlib (mochiOS サービス) 専用のシンボル。
        // 外部バイナリ (BusyBox 等) はシンボルテーブルが巨大で探索コストが高いためスキップ。
        let needs_sinit = exec_path.ends_with(".service");
        let mut sinit_addr: Option<u64> = None;
        if needs_sinit {
            if let Some(eh_sym) = elf_loader::parse_elf_header(data) {
                let shoff = eh_sym.e_shoff as usize;
                let shentsz = eh_sym.e_shentsize as usize;
                let shnum = eh_sym.e_shnum as usize;
                if shoff > 0 && shentsz > 0 && shnum > 0 && data.len() >= shoff + shentsz * shnum {
                    let mut symtab_offset: usize = 0;
                    let mut symtab_size: usize = 0;
                    let mut symtab_entsize: usize = 0;
                    let mut strtab_offset: usize = 0;
                    let mut strtab_size: usize = 0;
                    for si in 0..shnum {
                        let sh_off = shoff + si * shentsz;
                        if sh_off + shentsz > data.len() {
                            break;
                        }
                        let sh_type = match data[sh_off + 4..sh_off + 8].try_into() {
                            Ok(b) => u32::from_le_bytes(b),
                            Err(_) => {
                                crate::warn!("ELF section header truncated");
                                return crate::syscall::types::EINVAL;
                            }
                        };
                        let sh_offset = match data[sh_off + 24..sh_off + 32].try_into() {
                            Ok(b) => u64::from_le_bytes(b) as usize,
                            Err(_) => {
                                crate::warn!("ELF section header truncated");
                                return crate::syscall::types::EINVAL;
                            }
                        };
                        let sh_size = match data[sh_off + 32..sh_off + 40].try_into() {
                            Ok(b) => u64::from_le_bytes(b) as usize,
                            Err(_) => {
                                crate::warn!("ELF section header truncated");
                                return crate::syscall::types::EINVAL;
                            }
                        };
                        let sh_link = match data[sh_off + 40..sh_off + 44].try_into() {
                            Ok(b) => u32::from_le_bytes(b),
                            Err(_) => {
                                crate::warn!("ELF section header truncated");
                                return crate::syscall::types::EINVAL;
                            }
                        };
                        let sh_entsize = match data[sh_off + 56..sh_off + 64].try_into() {
                            Ok(b) => u64::from_le_bytes(b) as usize,
                            Err(_) => {
                                crate::warn!("ELF section header truncated");
                                return crate::syscall::types::EINVAL;
                            }
                        };

                        // SHT_SYMTAB == 2
                        if sh_type == 2 {
                            symtab_offset = sh_offset;
                            symtab_size = sh_size;
                            symtab_entsize = sh_entsize;
                            // linked string table
                            let link_idx = sh_link as usize;
                            if link_idx < shnum {
                                let link_sh_off = shoff + link_idx * shentsz;
                                strtab_offset =
                                    match data[link_sh_off + 24..link_sh_off + 32].try_into() {
                                        Ok(b) => u64::from_le_bytes(b) as usize,
                                        Err(_) => {
                                            crate::warn!("ELF section header truncated");
                                            return crate::syscall::types::EINVAL;
                                        }
                                    };
                                strtab_size =
                                    match data[link_sh_off + 32..link_sh_off + 40].try_into() {
                                        Ok(b) => u64::from_le_bytes(b) as usize,
                                        Err(_) => {
                                            crate::warn!("ELF section header truncated");
                                            return crate::syscall::types::EINVAL;
                                        }
                                    };
                            }
                            break;
                        }
                    }
                    if symtab_offset > 0 && strtab_offset > 0 && symtab_entsize > 0 {
                        let nsyms = symtab_size / symtab_entsize;
                        for i_sym in 0..nsyms {
                            let sym_off = symtab_offset + i_sym * symtab_entsize;
                            if sym_off + symtab_entsize > data.len() {
                                break;
                            }
                            let st_name = match data[sym_off..sym_off + 4].try_into() {
                                Ok(b) => u32::from_le_bytes(b) as usize,
                                Err(_) => {
                                    crate::warn!("ELF symbol entry truncated");
                                    break;
                                }
                            };
                            let st_value = match data[sym_off + 8..sym_off + 16].try_into() {
                                Ok(b) => u64::from_le_bytes(b),
                                Err(_) => {
                                    crate::warn!("ELF symbol entry truncated");
                                    break;
                                }
                            };

                            if st_name < strtab_size {
                                let name_off = strtab_offset + st_name;
                                if name_off < data.len() {
                                    let mut end = name_off;
                                    while end < data.len() && data[end] != 0 {
                                        end += 1;
                                    }
                                    if end <= data.len() {
                                        if let Ok(name_str) =
                                            core::str::from_utf8(&data[name_off..end])
                                        {
                                            if name_str == "__sinit" {
                                                sinit_addr = Some(st_value);
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        } // needs_sinit

        let base_name = exec_path.rsplit('/').next().unwrap_or(process_name);
        let argv0 = base_name.strip_suffix(".elf").unwrap_or(base_name);
        let mut all_args: Vec<&str> = Vec::new();
        all_args.push(argv0);
        for a in args {
            all_args.push(a);
        }
        if process_name.ends_with("busybox.elf") {
            let argv1 = all_args.get(1).copied().unwrap_or("");
            crate::info!(
                "busybox argv: argc={}, argv0='{}', argv1='{}'",
                all_args.len(),
                argv0,
                argv1
            );
        }
        let envs: [&str; 0] = [];
        let auxv_entries = [
            (3u64, phdr_vaddr),
            (4u64, phentsize),
            (5u64, phnum),
            (6u64, 4096u64),
            (7u64, 0u64),
            (8u64, 0u64),
            (9u64, entry),
            (11u64, 0u64),
            (12u64, 0u64),
            (13u64, 0u64),
            (14u64, 0u64),
            (16u64, 0u64),
            (17u64, 100u64),
            (23u64, 0u64),
            (25u64, 0u64),
            (31u64, 0u64),
            (0u64, 0u64),
        ];
        let InitialUserStack {
            stack_base_vaddr,
            stack_end_vaddr,
            initial_rsp,
            page_data,
        } = match build_initial_user_stack(aslr_seed, &all_args, &envs, exec_path, &auxv_entries) {
            Ok(stack) => stack,
            Err(errno) => return errno,
        };

        crate::debug!(
            "Allocating user stack: base={:#x}, top={:#x}, size={} pages, rsp={:#x}",
            stack_base_vaddr,
            stack_end_vaddr,
            USER_STACK_SIZE_PAGES,
            initial_rsp
        );

        // Map the lower 7 pages as zero-filled (writable, non-executable stack)
        if let Err(e) = crate::mem::paging::map_and_copy_segment_to(
            new_pt_phys,
            stack_base_vaddr,
            0,
            (USER_STACK_SIZE_PAGES - 1) as u64 * 4096,
            &[],
            true,
            false,
        ) {
            crate::warn!("Failed to allocate user stack lower: {:?}", e);
            return crate::syscall::types::EINVAL;
        }
        // Map the top page with args (writable, non-executable stack)
        let top_page_vaddr = stack_end_vaddr - 4096;
        if let Err(e) = crate::mem::paging::map_and_copy_segment_to(
            new_pt_phys,
            top_page_vaddr,
            4096,
            4096,
            &page_data,
            true,
            false,
        ) {
            crate::warn!("Failed to allocate user stack top: {:?}", e);
            return crate::syscall::types::EINVAL;
        }

        crate::debug!("User stack allocated successfully");

        // Pre-map initial heap pages to avoid immediate page faults from user allocations.
        // Map two pages at the default heap base so small early allocations won't fault.
        const HEAP_BASE_MIN: u64 = 0x4000_0000;
        const HEAP_ASLR_MAX_PAGES: u64 = 0x8000; // 128MiB
        let default_heap_base = HEAP_BASE_MIN
            .saturating_add(aslr_offset_pages(aslr_seed ^ 0x4a11_6b5c, HEAP_ASLR_MAX_PAGES) * 4096);
        let heap_map_size: u64 = 4096 * 2;
        let mut heap_pre_mapped = false;
        if let Err(e) = crate::mem::paging::map_and_copy_segment_to(
            new_pt_phys,
            default_heap_base,
            0,
            heap_map_size,
            &[],
            true,
            false,
        ) {
            crate::warn!(
                "Failed to pre-map initial heap pages at {:#x}: {:?}",
                default_heap_base,
                e
            );
        } else {
            crate::debug!(
                "Pre-mapped {} bytes for heap at {:#x} for {}",
                heap_map_size,
                default_heap_base,
                process_name
            );
            heap_pre_mapped = true;
        }

        // __sinitがあれば、スタブを作成して先に呼び出す
        if let Some(sinit) = sinit_addr {
            let stub_addr = match stack_end_vaddr.checked_add(4096) {
                Some(v) => v,
                None => return crate::syscall::types::EINVAL,
            };
            crate::info!(
                "Found __sinit at {:#x}, mapping init stub at {:#x}",
                sinit,
                stub_addr
            );
            let mut stub_page = vec![0u8; 4096];
            let mut cur = 0usize;
            if cur + 24 > stub_page.len() {
                crate::warn!("__sinit stub size overflow: {}", cur + 24);
                return crate::syscall::types::EINVAL;
            }
            // movabs rax, <sinit>
            stub_page[cur..cur + 2].copy_from_slice(&[0x48, 0xB8]);
            cur += 2;
            stub_page[cur..cur + 8].copy_from_slice(&sinit.to_le_bytes());
            cur += 8;
            // call rax
            stub_page[cur..cur + 2].copy_from_slice(&[0xFF, 0xD0]);
            cur += 2;
            // movabs rax, <entry>
            stub_page[cur..cur + 2].copy_from_slice(&[0x48, 0xB8]);
            cur += 2;
            stub_page[cur..cur + 8].copy_from_slice(&entry.to_le_bytes());
            cur += 8;
            // jmp rax
            stub_page[cur..cur + 2].copy_from_slice(&[0xFF, 0xE0]);
            cur += 2;

            if let Err(e) = crate::mem::paging::map_and_copy_segment_to(
                new_pt_phys,
                stub_addr,
                cur as u64,
                4096,
                &stub_page[0..cur],
                false,
                true,
            ) {
                crate::warn!("Failed to map __sinit stub at {:#x}: {:?}", stub_addr, e);
            } else {
                // jump to stub first
                entry = stub_addr;
            }
        }

        // プロセスを作成してページテーブルをセット
        let parent_pid = parent_override.or_else(|| {
            crate::task::current_thread_id()
                .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
        });
        let privilege = resolve_exec_privilege(process_name, exec_path);
        let mut proc = crate::task::Process::new(process_name, privilege, parent_pid, 0);
        proc.set_page_table(new_pt_phys);
        proc.set_stack_bottom(stack_base_vaddr);
        proc.set_stack_top(stack_end_vaddr);
        // 親プロセスの CWD を子プロセスに継承する
        if let Some(ppid) = parent_pid {
            let parent_cwd = crate::task::with_process(ppid, |p| {
                let mut s = alloc::string::String::new();
                s.push_str(p.cwd());
                s
            });
            if let Some(cwd_str) = parent_cwd {
                proc.set_cwd(&cwd_str);
            }
        }
        if heap_pre_mapped {
            proc.set_heap_start(default_heap_base);
            proc.set_heap_end(default_heap_base + heap_map_size);
        }
        let initial_fs_base = match map_initial_tls(new_pt_phys, aslr_seed) {
            Ok(base) => base,
            Err(errno) => return errno,
        };
        let pid = proc.id();
        let is_core_service = process_name.ends_with("core.service");
        if is_core_service
            && SERVICE_MANAGER_PID
                .compare_exchange(0, pid.as_u64(), Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
        {
            crate::warn!("core.service is already running, rejecting duplicate launch");
            return crate::syscall::types::EINVAL;
        }
        if crate::task::add_process(proc).is_none() {
            if is_core_service {
                let _ = SERVICE_MANAGER_PID.compare_exchange(
                    pid.as_u64(),
                    0,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
            }
            return crate::syscall::types::EINVAL;
        }
        new_pt_guard.disarm();
        // allocate kernel stack for the new thread
        const KERNEL_THREAD_STACK_SIZE: usize = 4096 * 32; // 128KB
        let kstack = match crate::task::thread::allocate_kernel_stack(KERNEL_THREAD_STACK_SIZE) {
            Some(a) => a,
            None => {
                crate::warn!("Failed to allocate kernel stack for thread");
                let _ = crate::task::remove_process(pid);
                if is_core_service {
                    let _ = SERVICE_MANAGER_PID.compare_exchange(
                        pid.as_u64(),
                        0,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    );
                }
                let _ = crate::mem::paging::destroy_user_page_table(new_pt_phys);
                return crate::syscall::types::ENOMEM;
            }
        };

        // ユーザーモードスレッドを作成
        // RSP に initial_rsp を設定
        let mut thread = crate::task::Thread::new_usermode(
            pid,
            process_name,
            entry,
            initial_rsp,
            kstack,
            KERNEL_THREAD_STACK_SIZE,
        );
        thread.set_fs_base(initial_fs_base);

        crate::info!(
            "exec: loaded '{}', entry={:#x}, pid={:?}",
            process_name,
            entry,
            pid
        );

        if crate::task::add_thread(thread).is_none() {
            crate::warn!("Failed to add thread");
            let _ = crate::task::remove_process(pid);
            if is_core_service {
                let _ = SERVICE_MANAGER_PID.compare_exchange(
                    pid.as_u64(),
                    0,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
            }
            let _ = crate::mem::paging::destroy_user_page_table(new_pt_phys);
            return crate::syscall::types::EINVAL;
        }

        crate::debug!(
            "exec: created usermode process '{}' (pid={:?}, entry={:#x})",
            process_name,
            pid,
            entry
        );

        pid.as_u64()
    }
}

/// ユーザー空間の null 終端ポインタ配列（char**）を読み取る
///
/// 各エントリは 64 ビットポインタ。NULL で終端。
/// max_entries を超えた場合は切り捨てる。
fn read_user_ptr_array(array_ptr: u64, max_entries: usize) -> Vec<String> {
    use crate::syscall::types::EFAULT;
    if array_ptr == 0 {
        return Vec::new();
    }
    let mut result = Vec::new();
    for i in 0..=max_entries {
        let ptr_addr = match (i as u64)
            .checked_mul(8)
            .and_then(|o| array_ptr.checked_add(o))
        {
            Some(a) => a,
            None => break,
        };
        if !crate::syscall::validate_user_ptr(ptr_addr, 8) {
            break;
        }
        let entry_ptr = crate::syscall::with_user_memory_access(|| unsafe {
            core::ptr::read_unaligned(ptr_addr as *const u64)
        });
        if entry_ptr == 0 {
            break;
        }
        let s = match crate::syscall::read_user_cstring(entry_ptr, 4096) {
            Ok(s) => s,
            Err(_) => break,
        };
        result.push(s);
        if result.len() >= max_entries {
            break;
        }
    }
    result
}

/// execve システムコール
///
/// 現在のプロセスイメージを新しいプログラムで置き換える
///
/// # 引数
/// - `path_ptr`: 実行ファイルパスのポインタ (null 終端)
/// - `argv`: 引数ポインタ配列 (char*[]) — null 終端、0 の場合は [path] を使用
/// - `envp`: 環境変数ポインタ配列 (char*[]) — null 終端、0 の場合は空
pub fn execve_syscall(path_ptr: u64, argv: u64, envp: u64) -> u64 {
    use crate::syscall::types::{EINVAL, ENOENT, EPERM};

    if path_ptr == 0 {
        return EINVAL;
    }

    let path_owned = match crate::syscall::read_user_cstring(path_ptr, 256) {
        Ok(s) => s,
        Err(_) => return EINVAL,
    };
    let path = path_owned.as_str();
    let aslr_seed = next_aslr_seed(path);

    // サービス起動はサービスマネージャー(Coreまたは登録PID)に限定
    if path.ends_with(".service") && !caller_can_launch_service() {
        return EPERM;
    }

    // initfs からファイルを読み込む
    let data_vec = match crate::init::fs::read(path) {
        Some(d) => d,
        None => return ENOENT,
    };
    let data: &[u8] = &data_vec;

    // ELF エントリポイントとセグメントを解析
    let entry = match crate::elf::loader::entry_point(data) {
        Some(e) => e,
        None => return EINVAL,
    };

    // 新しいページテーブルを作成
    let new_pt_phys = match crate::mem::paging::create_user_page_table() {
        Ok(p) => p,
        Err(_) => return EINVAL,
    };
    let mut new_pt_guard = UserPageTableGuard::new(new_pt_phys);

    // PT_LOAD セグメントをマップ / ELF メタデータを収集
    const USER_SPACE_END_EXECVE: u64 = 0x0000_7FFF_FFFF_FFFF;
    let mut phdr_vaddr: u64 = 0;
    let mut phentsize: u64 = 0;
    let mut phnum: u64 = 0;
    if let Some(eh) = crate::elf::loader::parse_elf_header(data) {
        // ELFアーキテクチャ検証 (MED-07)
        if eh.e_machine != EM_X86_64 {
            crate::warn!("execve: ELF e_machine {:#x} is not x86-64", eh.e_machine);
            return EINVAL;
        }
        phentsize = eh.e_phentsize as u64;
        phnum = eh.e_phnum as u64;
        let phoff = eh.e_phoff as usize;
        let phentsz = eh.e_phentsize as usize;
        // phentszが0の場合は無限ループを防ぐ (MED-08)
        if phentsz == 0 {
            return EINVAL;
        }
        let n = eh.e_phnum as usize;
        let mut load_base: u64 = 0;
        let mut load_base_set = false;
        for i in 0..n {
            // オーバーフロー安全な乗算と加算 (MED-08)
            let off_hdr = match i.checked_mul(phentsz).and_then(|x| phoff.checked_add(x)) {
                Some(o) if o < data.len() => o,
                _ => return EINVAL,
            };
            if let Some(ph) = crate::elf::loader::parse_phdr(data, off_hdr) {
                if ph.p_type == crate::elf::loader::PT_LOAD {
                    // ELFセグメントのvaddrがユーザー空間内であることを検証 (CRIT-05)
                    if ph.p_vaddr >= USER_SPACE_END_EXECVE {
                        crate::warn!(
                            "execve: ELF segment vaddr {:#x} is in kernel space",
                            ph.p_vaddr
                        );
                        return EINVAL;
                    }
                    if ph.p_memsz > 0 {
                        match ph.p_vaddr.checked_add(ph.p_memsz) {
                            Some(e) if e <= USER_SPACE_END_EXECVE => {}
                            _ => {
                                crate::warn!(
                                    "execve: ELF segment vaddr+memsz overflows user space"
                                );
                                return EINVAL;
                            }
                        }
                    }
                    // 最初の PT_LOAD から load_base を計算 (AT_PHDR 算出用)
                    if !load_base_set {
                        load_base = ph.p_vaddr.saturating_sub(ph.p_offset);
                        load_base_set = true;
                    }
                    // ELFセグメントの境界チェック (CRIT-04)
                    let src_off = ph.p_offset as usize;
                    let src_end = match src_off.checked_add(ph.p_filesz as usize) {
                        Some(e) if e <= data.len() => e,
                        _ => {
                            crate::warn!("execve: ELF segment src offset+filesz out of bounds");
                            return EINVAL;
                        }
                    };
                    let seg_src = &data[src_off..src_end];
                    if crate::mem::paging::map_and_copy_segment_to(
                        new_pt_phys,
                        ph.p_vaddr,
                        ph.p_filesz,
                        ph.p_memsz,
                        seg_src,
                        (ph.p_flags & 0x2) != 0,
                        (ph.p_flags & 0x1) != 0,
                    )
                    .is_err()
                    {
                        return EINVAL;
                    }
                }
            }
        }
        phdr_vaddr = load_base + eh.e_phoff;
    }

    // ユーザースタックをセットアップ (Linux x86_64 ABI: argc, argv[], NULL, envp[], NULL, auxv[])
    // argv / envp をユーザー空間から読み込む
    let mut argv_strings = read_user_ptr_array(argv, 256);
    if argv_strings.is_empty() {
        argv_strings.push(path_owned.clone());
    }
    let envp_strings = read_user_ptr_array(envp, 1024);
    let argc = argv_strings.len();
    let argv_refs: Vec<&str> = argv_strings.iter().map(|s| s.as_str()).collect();
    let envp_refs: Vec<&str> = envp_strings.iter().map(|s| s.as_str()).collect();
    let auxv_entries = [
        (3u64, phdr_vaddr),
        (4u64, phentsize),
        (5u64, phnum),
        (6u64, 4096u64),
        (7u64, 0u64),
        (8u64, 0u64),
        (9u64, entry),
        (11u64, 0u64),
        (12u64, 0u64),
        (13u64, 0u64),
        (14u64, 0u64),
        (16u64, 0u64),
        (17u64, 100u64),
        (23u64, 0u64),
        (25u64, 0u64),
        (31u64, 0u64),
        (0u64, 0u64),
    ];
    let InitialUserStack {
        stack_base_vaddr,
        stack_end_vaddr,
        initial_rsp,
        page_data,
    } = match build_initial_user_stack(
        aslr_seed,
        &argv_refs,
        &envp_refs,
        &path_owned,
        &auxv_entries,
    ) {
        Ok(stack) => stack,
        Err(errno) => return errno,
    };

    if crate::mem::paging::map_and_copy_segment_to(
        new_pt_phys,
        stack_base_vaddr,
        0,
        (USER_STACK_SIZE_PAGES - 1) as u64 * 4096,
        &[],
        true,
        false,
    )
    .is_err()
    {
        return EINVAL;
    }
    let top_page_vaddr = stack_end_vaddr - 4096;
    if crate::mem::paging::map_and_copy_segment_to(
        new_pt_phys,
        top_page_vaddr,
        4096,
        4096,
        &page_data,
        true,
        false,
    )
    .is_err()
    {
        return EINVAL;
    }

    // 初期ヒープをASLR付きで確保
    const HEAP_BASE_MIN: u64 = 0x4000_0000;
    const HEAP_ASLR_MAX_PAGES: u64 = 0x8000; // 128MiB
    let heap_base = HEAP_BASE_MIN
        .saturating_add(aslr_offset_pages(aslr_seed ^ 0x4a11_6b5c, HEAP_ASLR_MAX_PAGES) * 4096);
    let heap_map_size: u64 = 4096 * 2;
    if crate::mem::paging::map_and_copy_segment_to(
        new_pt_phys,
        heap_base,
        0,
        heap_map_size,
        &[],
        true,
        false,
    )
    .is_err()
    {
        return EINVAL;
    }
    let initial_fs_base = match map_initial_tls(new_pt_phys, aslr_seed) {
        Ok(base) => base,
        Err(errno) => return errno,
    };

    // 現在のプロセスのページテーブルとヒープを更新
    let current_tid = match crate::task::current_thread_id() {
        Some(t) => t,
        None => return EINVAL,
    };
    let pid = match crate::task::with_thread(current_tid, |t| t.process_id()) {
        Some(p) => p,
        None => return EINVAL,
    };
    crate::task::with_thread_mut(current_tid, |t| t.set_fs_base(initial_fs_base));
    let old_pt_phys = crate::task::with_process_mut(pid, |p| {
        let prev = p.page_table();
        p.set_page_table(new_pt_phys);
        p.set_heap_start(heap_base);
        p.set_heap_end(heap_base + heap_map_size);
        p.set_stack_bottom(stack_base_vaddr);
        p.set_stack_top(stack_end_vaddr);
        prev
    })
    .flatten();
    if let Some(old) = old_pt_phys {
        if old != new_pt_phys {
            let _ = crate::mem::paging::destroy_user_page_table(old);
        }
    }
    new_pt_guard.disarm();

    // FD_CLOEXEC が設定された FD を exec 時に閉じる
    crate::task::with_process_mut(pid, |p| p.fd_table_mut().close_cloexec_fds());

    // 新しいページテーブルに切り替えてジャンプ
    unsafe {
        crate::mem::paging::switch_page_table(new_pt_phys);
        crate::task::jump_to_usermode(entry, initial_rsp);
    }
}

/// メモリ上の ELF バッファから新プロセスを起動するシステムコール
///
/// # 引数
/// - `buf_ptr`: ユーザー空間の ELF データへのポインタ
/// - `buf_len`: バッファのバイト数
pub fn exec_from_buffer_syscall(buf_ptr: u64, buf_len: u64) -> u64 {
    use crate::syscall::types::{EFAULT, EINVAL, EPERM};

    // core/service のみ許可
    if !caller_can_launch_service() {
        return EPERM;
    }

    if buf_ptr == 0 || buf_len == 0 || buf_len > 32 * 1024 * 1024 {
        return EINVAL;
    }

    // ポインタの範囲がユーザー空間内かつ現在のプロセスのページテーブルにマップ済みか検証
    if !crate::syscall::validate_user_ptr(buf_ptr, buf_len) {
        return EFAULT;
    }

    // KPTI 環境ではカーネルは kernel CR3 で動作しており、ユーザー空間の
    // 仮想アドレスに直接アクセスできない。
    // with_user_memory_access でユーザー CR3 に一時切替してバルクコピーする。
    let mut owned = alloc::vec![0u8; buf_len as usize];
    let dst_ptr = owned.as_mut_ptr();
    crate::syscall::with_user_memory_access(|| unsafe {
        core::ptr::copy_nonoverlapping(buf_ptr as *const u8, dst_ptr, buf_len as usize);
    });

    exec_with_data(
        &owned,
        "user_exec",
        "user_exec",
        &[],
        delegated_parent_pid(),
    )
}

/// メモリ上の ELF バッファと実行パス名から新プロセスを起動するシステムコール
///
/// # 引数
/// - `buf_ptr`: ユーザー空間の ELF データへのポインタ
/// - `buf_len`: バッファのバイト数
/// - `path_ptr`: ユーザー空間の null 終端パス文字列
pub fn exec_from_buffer_named_syscall(buf_ptr: u64, buf_len: u64, path_ptr: u64) -> u64 {
    use crate::syscall::types::{EFAULT, EINVAL, EPERM};

    if !caller_can_launch_service() {
        return EPERM;
    }
    if buf_ptr == 0 || buf_len == 0 || buf_len > 32 * 1024 * 1024 || path_ptr == 0 {
        return EINVAL;
    }
    if !crate::syscall::validate_user_ptr(buf_ptr, buf_len) {
        return EFAULT;
    }

    let path = match crate::syscall::read_user_cstring(path_ptr, 256) {
        Ok(s) => s,
        Err(_) => return EINVAL,
    };
    let process_name = path.rsplit('/').next().unwrap_or(path.as_str());

    let mut owned = alloc::vec![0u8; buf_len as usize];
    let dst_ptr = owned.as_mut_ptr();
    crate::syscall::with_user_memory_access(|| unsafe {
        core::ptr::copy_nonoverlapping(buf_ptr as *const u8, dst_ptr, buf_len as usize);
    });

    exec_with_data(
        &owned,
        process_name,
        path.as_str(),
        &[],
        delegated_parent_pid(),
    )
}

/// メモリ上の ELF バッファと実行パス名・引数から新プロセスを起動するシステムコール
///
/// # 引数
/// - `buf_ptr`: ユーザー空間の ELF データへのポインタ
/// - `buf_len`: バッファのバイト数
/// - `path_ptr`: ユーザー空間の null 終端パス文字列
/// - `args_ptr`: ユーザー空間の null 区切り引数列（"arg1\0arg2\0\0"）
pub fn exec_from_buffer_named_args_syscall(
    buf_ptr: u64,
    buf_len: u64,
    path_ptr: u64,
    args_ptr: u64,
) -> u64 {
    use crate::syscall::types::{EFAULT, EINVAL, EPERM};

    if !caller_can_launch_service() {
        return EPERM;
    }
    if buf_ptr == 0 || buf_len == 0 || buf_len > 32 * 1024 * 1024 || path_ptr == 0 {
        return EINVAL;
    }
    if !crate::syscall::validate_user_ptr(buf_ptr, buf_len) {
        return EFAULT;
    }

    let path = match crate::syscall::read_user_cstring(path_ptr, 256) {
        Ok(s) => s,
        Err(_) => return EINVAL,
    };
    let process_name = path.rsplit('/').next().unwrap_or(path.as_str());

    let args_owned = match read_nul_args_from_user(args_ptr, 512, 64) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let args_refs: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();

    let mut owned = alloc::vec![0u8; buf_len as usize];
    let dst_ptr = owned.as_mut_ptr();
    crate::syscall::with_user_memory_access(|| unsafe {
        core::ptr::copy_nonoverlapping(buf_ptr as *const u8, dst_ptr, buf_len as usize);
    });

    exec_with_data(
        &owned,
        process_name,
        path.as_str(),
        &args_refs,
        delegated_parent_pid(),
    )
}

/// メモリ上の ELF バッファと実行パス名・引数・要求元スレッドIDから新プロセスを起動するシステムコール
pub fn exec_from_buffer_named_args_with_requester_syscall(
    buf_ptr: u64,
    buf_len: u64,
    path_ptr: u64,
    args_ptr: u64,
    requester_tid: u64,
) -> u64 {
    use crate::syscall::types::{EFAULT, EINVAL, EPERM};

    if !caller_can_launch_service() {
        return EPERM;
    }
    if buf_ptr == 0 || buf_len == 0 || buf_len > 32 * 1024 * 1024 || path_ptr == 0 {
        return EINVAL;
    }
    if !crate::syscall::validate_user_ptr(buf_ptr, buf_len) {
        return EFAULT;
    }

    let path = match crate::syscall::read_user_cstring(path_ptr, 256) {
        Ok(s) => s,
        Err(_) => return EINVAL,
    };
    let process_name = path.rsplit('/').next().unwrap_or(path.as_str());

    let args_owned = match read_nul_args_from_user(args_ptr, 512, 64) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let args_refs: Vec<&str> = args_owned.iter().map(|s| s.as_str()).collect();

    let mut owned = alloc::vec![0u8; buf_len as usize];
    let dst_ptr = owned.as_mut_ptr();
    crate::syscall::with_user_memory_access(|| unsafe {
        core::ptr::copy_nonoverlapping(buf_ptr as *const u8, dst_ptr, buf_len as usize);
    });

    let parent_override = if requester_tid != 0 {
        let requester = crate::task::ThreadId::from_u64(requester_tid);
        let caller_pid = match crate::task::current_thread_id()
            .and_then(|tid| crate::task::with_thread(tid, |t| t.process_id()))
        {
            Some(pid) => pid,
            None => return EPERM,
        };
        match crate::task::with_thread(requester, |t| t.process_id()) {
            Some(pid) => {
                let caller_is_core =
                    crate::task::with_process(caller_pid, |p| {
                        p.privilege() == crate::task::PrivilegeLevel::Core
                    })
                    .unwrap_or(false);

                if pid != caller_pid && !caller_is_core {
                    return EPERM;
                }
                Some(pid)
            }
            None => return EINVAL,
        }
    } else {
        None
    };

    exec_with_data(
        &owned,
        process_name,
        path.as_str(),
        &args_refs,
        parent_override.or_else(delegated_parent_pid),
    )
}
