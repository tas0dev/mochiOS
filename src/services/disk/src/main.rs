use core::mem::size_of;
use core::sync::atomic::{AtomicBool, Ordering};

use swiftlib::ipc;
use swiftlib::task;

mod ata;

use ata::{AtaDrive, AtaPorts, DriveType};

const MAX_DISKS: usize = 4;
const MAX_BULK_READ_SECTORS: u64 = 32;

static mut DISKS: [Option<AtaDrive>; MAX_DISKS] = [None, None, None, None];
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// ディスク操作リクエスト（書き込みデータを含む）
#[repr(C)]
#[derive(Clone, Copy)]
struct DiskRequest {
    op: u64,
    disk_id: u64,
    lba: u64,
    count: u64,
    data: [u8; 512], // OP_WRITE のときに使用
}

impl DiskRequest {
    const OP_READ: u64 = 1;
    const OP_WRITE: u64 = 2;
    const OP_INFO: u64 = 3;
    const OP_LIST: u64 = 4;
}

/// サービス準備完了通知
#[repr(C)]
#[derive(Clone, Copy)]
struct ReadyNotify {
    op: u64, // OP_NOTIFY_READY
}

const OP_NOTIFY_READY: u64 = 0xFF;

/// ディスク操作レスポンス
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DiskResponse {
    status: i64,
    len: u64,
    data: [u8; 512],
}

#[repr(align(8))]
struct AlignedBuffer([u8; 544]); // DiskRequest は 544 バイト

/// ディスクドライバを初期化
fn init_disks() {
    println!("[DISK] Initializing ATA drives...");

    unsafe {
        // Primary Master
        let mut drive0 = AtaDrive::new(AtaPorts::PRIMARY, DriveType::Master);
        if drive0.init().is_ok() {
            println!(
                "[DISK] Primary Master detected: {} sectors",
                drive0.sector_count()
            );
            DISKS[0] = Some(drive0);
        } else {
            println!("[DISK] Primary Master not found");
        }

        // Primary Slave
        let mut drive1 = AtaDrive::new(AtaPorts::PRIMARY, DriveType::Slave);
        if drive1.init().is_ok() {
            println!(
                "[DISK] Primary Slave detected: {} sectors",
                drive1.sector_count()
            );
            DISKS[1] = Some(drive1);
        } else {
            println!("[DISK] Primary Slave not found");
        }

        // Secondary Master
        let mut drive2 = AtaDrive::new(AtaPorts::SECONDARY, DriveType::Master);
        if drive2.init().is_ok() {
            println!(
                "[DISK] Secondary Master detected: {} sectors",
                drive2.sector_count()
            );
            DISKS[2] = Some(drive2);
        } else {
            println!("[DISK] Secondary Master not found");
        }

        // Secondary Slave
        let mut drive3 = AtaDrive::new(AtaPorts::SECONDARY, DriveType::Slave);
        if drive3.init().is_ok() {
            println!(
                "[DISK] Secondary Slave detected: {} sectors",
                drive3.sector_count()
            );
            DISKS[3] = Some(drive3);
        } else {
            println!("[DISK] Secondary Slave not found");
        }
    }

    INITIALIZED.store(true, Ordering::Release);
    println!("[DISK] ATA initialization complete");
}

/// core.service に準備完了を通知する
fn notify_ready_to_core() {
    // core.service の PID を取得
    let core_pid = match task::find_process_by_name("core.service") {
        Some(pid) => {
            println!("[DISK] Found core.service (PID={})", pid);
            pid
        }
        None => {
            println!("[DISK] WARNING: core.service not found, skipping READY notify");
            return;
        }
    };

    let notify = ReadyNotify { op: OP_NOTIFY_READY };
    let notify_slice = unsafe {
        core::slice::from_raw_parts(
            &notify as *const _ as *const u8,
            size_of::<ReadyNotify>(),
        )
    };
    if ipc::ipc_send(core_pid, notify_slice) == 0 {
        println!("[DISK] Sent READY to core.service (PID={})", core_pid);
    } else {
        println!("[DISK] Failed to send READY to core.service");
    }
}

#[inline]
fn send_response(dest_thread: u64, resp: &DiskResponse) {
    let resp_slice = unsafe {
        core::slice::from_raw_parts(resp as *const _ as *const u8, size_of::<DiskResponse>())
    };
    let _ = ipc::ipc_send(dest_thread, resp_slice);
}

#[allow(static_mut_refs)]
fn main() {
    println!("[DISK] Disk I/O Service Started.");

    // ディスクを初期化
    init_disks();

    // 初期化完了を core.service へ通知
    notify_ready_to_core();

    let mut recv_buf = AlignedBuffer([0u8; 544]);

    loop {
        let (sender, len) = ipc::ipc_recv(&mut recv_buf.0);

        // メッセージなし（ipc_recv の戻り値は (0, 0)）
        if sender == 0 && len == 0 {
            task::yield_now();
            continue;
        }

        if sender != 0 && (len as usize) >= (size_of::<DiskRequest>() - 512) {
            // 送信元スレッドの権限を確認 (#22: 非特権プロセスからのディスクアクセスを拒否)
            // 0=Core, 1=Service のみ許可。2=User は拒否
            let sender_privilege = task::get_thread_privilege(sender);
            if sender_privilege > 1 {
                println!("[DISK] Rejecting request from unprivileged thread {}", sender);
                let resp = DiskResponse { status: -1, len: 0, data: [0; 512] };
                send_response(sender, &resp);
                continue;
            }

            let req: DiskRequest = unsafe { core::ptr::read_unaligned(recv_buf.0.as_ptr() as *const _) };
            if req.op != DiskRequest::OP_READ {
                println!(
                    "[DISK] REQ op={} disk={} lba={} from PID={}",
                    req.op, req.disk_id, req.lba, sender
                );
            }

            let mut resp = DiskResponse {
                status: -1,
                len: 0,
                data: [0; 512],
            };

            match req.op {
                DiskRequest::OP_READ => {
                    let disk_id = req.disk_id as usize;
                    if req.count == 0 || req.count > MAX_BULK_READ_SECTORS {
                        resp.status = -22; // EINVAL
                    } else if disk_id < MAX_DISKS {
                        unsafe {
                            if let Some(ref drive) = DISKS[disk_id] {
                                let count = req.count as usize;
                                let total_bytes = match count.checked_mul(512) {
                                    Some(v) => v,
                                    None => {
                                        resp.status = -22; // EINVAL
                                        send_response(sender, &resp);
                                        continue;
                                    }
                                };
                                let mut bulk = vec![0u8; total_bytes];
                                match drive.read_sectors(req.lba, req.count as u8, &mut bulk) {
                                    Ok(_) => {
                                        for i in 0..count {
                                            let mut chunk_resp = DiskResponse {
                                                status: 0,
                                                len: 512,
                                                data: [0; 512],
                                            };
                                            let start = i * 512;
                                            let end = start + 512;
                                            chunk_resp.data.copy_from_slice(&bulk[start..end]);
                                            send_response(sender, &chunk_resp);
                                        }
                                    }
                                    Err(_) => {
                                        resp.status = -5; // EIO
                                        send_response(sender, &resp);
                                    }
                                };
                                continue;
                            } else {
                                resp.status = -6; // ENXIO (No such device)
                            }
                        }
                    } else {
                        resp.status = -22; // EINVAL
                    }
                }
                DiskRequest::OP_WRITE => {
                    let disk_id = req.disk_id as usize;
                    if disk_id < MAX_DISKS {
                        unsafe {
                            if let Some(ref mut drive) = DISKS[disk_id] {
                                match drive.write_sector(req.lba, &req.data) {
                                    Ok(_) => {
                                        resp.status = 0;
                                        resp.len = 0;
                                    }
                                    Err(_) => {
                                        resp.status = -5; // EIO
                                    }
                                }
                            } else {
                                resp.status = -6; // ENXIO
                            }
                        }
                    } else {
                        resp.status = -22; // EINVAL
                    }
                }
                DiskRequest::OP_INFO => {
                    let disk_id = req.disk_id as usize;
                    if disk_id < MAX_DISKS {
                        unsafe {
                            if let Some(ref drive) = DISKS[disk_id] {
                                let sectors = drive.sector_count();
                                // セクタ数を返す
                                resp.data[0..8].copy_from_slice(&sectors.to_le_bytes());
                                resp.status = 0;
                                resp.len = 8;
                            } else {
                                resp.status = -6; // ENXIO
                            }
                        }
                    } else {
                        resp.status = -22; // EINVAL
                    }
                }
                DiskRequest::OP_LIST => {
                    // 利用可能なディスクをリスト
                    let mut count = 0u8;
                    unsafe {
                        for (i, disk) in DISKS.iter().enumerate() {
                            if disk.is_some() {
                                resp.data[count as usize] = i as u8;
                                count += 1;
                            }
                        }
                    }
                    resp.status = 0;
                    resp.len = count as u64;
                }
                _ => {
                    println!("[DISK] Unknown OP: {}", req.op);
                    continue;
                }
            }

            send_response(sender, &resp);
        }
    }
}
