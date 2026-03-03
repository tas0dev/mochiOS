#![no_std]
#![no_main]

extern crate alloc;

use core::fmt::{self};
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, Ordering};

use swiftlib::io;
use swiftlib::ipc;
use swiftlib::task;

mod ata;

use ata::{AtaDrive, AtaPorts, DriveType};

const MAX_DISKS: usize = 4;

static mut DISKS: [Option<AtaDrive>; MAX_DISKS] = [None, None, None, None];
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// ディスク操作リクエスト
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DiskRequest {
    op: u64,
    disk_id: u64,
    lba: u64,
    count: u64,
}

impl DiskRequest {
    const OP_READ: u64 = 1;
    const OP_WRITE: u64 = 2;
    const OP_INFO: u64 = 3;
    const OP_LIST: u64 = 4;
}

/// ディスク操作レスポンス
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DiskResponse {
    status: i64,
    len: u64,
    data: [u8; 512],
}

#[repr(align(8))]
struct AlignedBuffer([u8; 1024]);

// 簡易的な標準出力ライター
struct Stdout;
impl fmt::Write for Stdout {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        io::write_stdout(s.as_bytes());
        Ok(())
    }
}

macro_rules! print {
    ($($arg:tt)*) => ({
        let _ = core::fmt::Write::write_fmt(&mut Stdout, format_args!($($arg)*));
    });
}

macro_rules! println {
    () => (print!("\n"));
    ($($arg:tt)*) => (print!("{}\n", format_args!($($arg)*)));
}

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

#[no_mangle]
#[allow(static_mut_refs)]
pub extern "C" fn main(_argc: i32, _argv: *const *const u8) -> i32 {
    println!("[DISK] Disk I/O Service Started.");

    // ディスクを初期化
    init_disks();

    let mut recv_buf = AlignedBuffer([0u8; 1024]);

    loop {
        let (sender, len) = ipc::ipc_recv(&mut recv_buf.0);

        // EAGAIN (メッセージなし) の場合はCPUを譲る
        if sender == 0xFFFFFFFF || len == 0xFFFFFFFD {
            task::yield_now();
            continue;
        }

        if sender != 0 && (len as usize) >= size_of::<DiskRequest>() {
            // 送信元スレッドの権限を確認 (#22: 非特権プロセスからのディスクアクセスを拒否)
            // 0=Core, 1=Service のみ許可。2=User は拒否
            let sender_privilege = task::get_thread_privilege(sender);
            if sender_privilege > 1 {
                println!("[DISK] Rejecting request from unprivileged thread {}", sender);
                let resp = DiskResponse { status: -1, len: 0, data: [0; 512] };
                let resp_slice = unsafe {
                    core::slice::from_raw_parts(
                        &resp as *const _ as *const u8,
                        size_of::<DiskResponse>(),
                    )
                };
                let _ = ipc::ipc_send(sender, resp_slice);
                continue;
            }

            let req: DiskRequest = unsafe { core::ptr::read(recv_buf.0.as_ptr() as *const _) };
            println!(
                "[DISK] REQ op={} disk={} lba={} from PID={}",
                req.op, req.disk_id, req.lba, sender
            );

            let mut resp = DiskResponse {
                status: -1,
                len: 0,
                data: [0; 512],
            };

            match req.op {
                DiskRequest::OP_READ => {
                    let disk_id = req.disk_id as usize;
                    if disk_id < MAX_DISKS {
                        unsafe {
                            if let Some(ref drive) = DISKS[disk_id] {
                                match drive.read_sector(req.lba, &mut resp.data) {
                                    Ok(_) => {
                                        resp.status = 0;
                                        resp.len = 512;
                                    }
                                    Err(_) => {
                                        resp.status = -5; // EIO
                                    }
                                }
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
                            if let Some(ref mut _drive) = DISKS[disk_id] {
                                // データは次のメッセージで受信する想定
                                // 簡略化のため未実装
                                resp.status = -38; // ENOSYS
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
                    resp.status = -38; // ENOSYS
                }
            }

            let resp_slice = unsafe {
                core::slice::from_raw_parts(
                    &resp as *const _ as *const u8,
                    size_of::<DiskResponse>(),
                )
            };

            let _ = ipc::ipc_send(sender, resp_slice);
        }
    }
}
