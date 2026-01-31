//! 物理フレームアロケータ
//!
//! 4KBページ単位で物理メモリを管理

use crate::{
    error::{KernelError, MemoryError, Result},
    MemoryRegion, MemoryType,
};
use spin::Mutex;
use x86_64::{
    structures::paging::{FrameAllocator, PhysFrame, Size4KiB},
    PhysAddr,
};

/// グローバルフレームアロケータ
pub static FRAME_ALLOCATOR: Mutex<Option<BitmapFrameAllocator>> = Mutex::new(None);

/// ビットマップベースのフレームアロケータ
pub struct BitmapFrameAllocator {
    /// メモリマップ
    memory_map: &'static [MemoryRegion],
    /// 次に割り当てるフレーム
    next_frame: usize,
}

impl BitmapFrameAllocator {
    /// 新しいフレームアロケータを作成
    pub fn new(memory_map: &'static [MemoryRegion]) -> Self {
        Self {
            memory_map,
            next_frame: {
                let mut start_idx = 0usize;
                for r in memory_map.iter() {
                    if r.region_type == MemoryType::Usable {
                        start_idx = (r.start / 4096) as usize;
                        break;
                    }
                }
                start_idx
            },
        }
    }

    /// 使用可能な物理メモリの総量を計算（バイト）
    pub fn usable_memory(&self) -> u64 {
        self.memory_map
            .iter()
            .filter(|r| r.region_type == MemoryType::Usable)
            .map(|r| r.len)
            .sum()
    }

    /// 使用可能なフレーム数を計算
    pub fn usable_frames(&self) -> usize {
        (self.usable_memory() / 4096) as usize
    }

    /// 使用可能なフレームのイテレータを返す
    fn usable_frames_iter(&self) -> impl Iterator<Item = PhysFrame> + '_ {
        self.memory_map
            .iter()
            .filter(|r| r.region_type == MemoryType::Usable)
            .flat_map(|r| {
                let start_addr = r.start;
                let end_addr = r.start + r.len;
                let start_frame = start_addr / 4096;
                let end_frame = end_addr / 4096;
                (start_frame..end_frame)
                    .map(|f| PhysFrame::containing_address(PhysAddr::new(f * 4096)))
            })
    }
}

unsafe impl FrameAllocator<Size4KiB> for BitmapFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        let frame = self.usable_frames_iter().nth(self.next_frame);
        self.next_frame += 1;
        frame
    }
}

/// フレームアロケータを初期化
pub fn init(memory_map: &'static [MemoryRegion]) {
    let allocator = BitmapFrameAllocator::new(memory_map);
    *FRAME_ALLOCATOR.lock() = Some(allocator);
}

/// フレームを割り当て
pub fn allocate_frame() -> Result<PhysFrame> {
    FRAME_ALLOCATOR
        .lock()
        .as_mut()
        .and_then(|a| a.allocate_frame())
        .ok_or(KernelError::Memory(MemoryError::OutOfMemory))
}

/// 使用可能なメモリ情報を取得
pub fn get_memory_info() -> Option<(u64, usize)> {
    FRAME_ALLOCATOR
        .lock()
        .as_ref()
        .map(|a| (a.usable_memory(), a.usable_frames()))
}
