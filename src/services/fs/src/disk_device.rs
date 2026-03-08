//! ディスクサービスを使用したブロックデバイス実装

use core::mem::size_of;

use swiftlib::ipc;

use crate::common::vfs::{VfsError, VfsResult};
use crate::ext2::BlockDevice;

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

#[allow(unused)]
impl DiskRequest {
    const OP_READ: u64 = 1;
    const OP_WRITE: u64 = 2;
    const OP_INFO: u64 = 3;
}

/// ディスク操作レスポンス
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct DiskResponse {
    status: i64,
    len: u64,
    data: [u8; 512],
}

/// ディスクサービスを使用したブロックデバイス
pub struct DiskServiceDevice {
    disk_service_pid: u64,
    disk_id: u64,
    sector_size: usize,
}

impl DiskServiceDevice {
    /// 新しいディスクデバイスを作成
    pub fn new(disk_service_pid: u64, disk_id: u64) -> Self {
        Self {
            disk_service_pid,
            disk_id,
            sector_size: 512,
        }
    }

    /// セクタを読み取る（内部用）
    fn read_sector(&self, lba: u64, buf: &mut [u8]) -> VfsResult<()> {
        if buf.len() < 512 {
            return Err(VfsError::InvalidArgument);
        }

        let req = DiskRequest {
            op: DiskRequest::OP_READ,
            disk_id: self.disk_id,
            lba,
            count: 1,
            data: [0u8; 512],
        };

        let req_slice = unsafe {
            core::slice::from_raw_parts(&req as *const _ as *const u8, size_of::<DiskRequest>())
        };

        // リクエストを送信
        let result = ipc::ipc_send(self.disk_service_pid, req_slice);
        if result != 0 {
            return Err(VfsError::IoError);
        }

        // レスポンスを受信（EAGAIN の場合はスピン、最大 1000 回）
        let mut resp_buf = [0u8; size_of::<DiskResponse>()];
        let (sender, len) = loop {
            let (s, l) = ipc::ipc_recv(&mut resp_buf);
            // EAGAIN sentinel: sender=0xFFFF_FFFF または len=0xFFFF_FFFD
            if s == 0xFFFF_FFFF || l == 0xFFFF_FFFD {
                continue;
            }
            break (s, l);
        };

        if sender != self.disk_service_pid || (len as usize) < size_of::<DiskResponse>() {
            return Err(VfsError::IoError);
        }

        let resp: DiskResponse = unsafe {
            core::ptr::read(resp_buf.as_ptr() as *const DiskResponse)
        };

        if resp.status != 0 {
            return Err(VfsError::IoError);
        }

        // データをコピー
        buf[..512].copy_from_slice(&resp.data);
        Ok(())
    }

    /// セクタに書き込む（内部用）
    fn write_sector(&self, lba: u64, buf: &[u8]) -> VfsResult<()> {
        if buf.len() < 512 {
            return Err(VfsError::InvalidArgument);
        }

        let mut req = DiskRequest {
            op: DiskRequest::OP_WRITE,
            disk_id: self.disk_id,
            lba,
            count: 1,
            data: [0u8; 512],
        };
        req.data.copy_from_slice(&buf[..512]);

        let req_slice = unsafe {
            core::slice::from_raw_parts(&req as *const _ as *const u8, size_of::<DiskRequest>())
        };

        let result = ipc::ipc_send(self.disk_service_pid, req_slice);
        if result != 0 {
            return Err(VfsError::IoError);
        }

        // レスポンスを受信（EAGAIN の場合はスピン）
        let mut resp_buf = [0u8; size_of::<DiskResponse>()];
        let (sender, len) = loop {
            let (s, l) = ipc::ipc_recv(&mut resp_buf);
            if s == 0xFFFF_FFFF || l == 0xFFFF_FFFD {
                continue;
            }
            break (s, l);
        };

        if sender != self.disk_service_pid || (len as usize) < size_of::<DiskResponse>() {
            return Err(VfsError::IoError);
        }

        let resp: DiskResponse = unsafe {
            core::ptr::read(resp_buf.as_ptr() as *const DiskResponse)
        };

        if resp.status != 0 {
            Err(VfsError::IoError)
        } else {
            Ok(())
        }
    }
}

impl BlockDevice for DiskServiceDevice {
    fn block_size(&self) -> usize {
        self.sector_size
    }

    fn read_block(&self, block_num: u64, buf: &mut [u8]) -> Result<(), ()> {
        self.read_sector(block_num, buf).map_err(|_| ())
    }

    fn write_block(&mut self, block_num: u64, buf: &[u8]) -> Result<(), ()> {
        self.write_sector(block_num, buf).map_err(|_| ())
    }
}
