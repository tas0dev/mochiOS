//! 物理フレームアロケータ
//!
//! 4KBページ単位で物理メモリを管理

use crate::{
    result::{Kernel, Memory, Result},
    MemoryRegion, MemoryType,
};
use spin::Mutex;
use x86_64::{
    structures::paging::{FrameAllocator, PhysFrame, Size4KiB},
    PhysAddr,
};

const RECYCLED_FRAME_CAP: usize = 4096;

/// グローバルフレームアロケータ
pub static FRAME_ALLOCATOR: Mutex<Option<BitmapFrameAllocator>> = Mutex::new(None);

/// ビットマップベースのフレームアロケータ
pub struct BitmapFrameAllocator {
    /// メモリマップ
    memory_map: &'static [MemoryRegion],
    /// 次に割り当てるフレーム
    next_frame: usize,
    /// 解放されたフレームの再利用スタック
    recycled_frames: [u64; RECYCLED_FRAME_CAP],
    recycled_count: usize,
}

impl BitmapFrameAllocator {
    /// 新しいフレームアロケータを作成
    ///
    /// ## Arguments
    /// - `memory_map`: ブートローダーから提供されたメモリマップ
    ///
    /// ## Returns
    /// 新しいフレームアロケータのインスタンス
    pub fn new(memory_map: &'static [MemoryRegion]) -> Self {
        Self {
            memory_map,
            next_frame: 0,
            recycled_frames: [0; RECYCLED_FRAME_CAP],
            recycled_count: 0,
        }
    }

    fn is_usable_frame_addr(&self, phys_addr: u64) -> bool {
        self.memory_map.iter().any(|r| {
            r.region_type == MemoryType::Usable
                && phys_addr >= r.start
                && phys_addr < r.start + r.len
        })
    }

    pub fn deallocate_frame(&mut self, frame: PhysFrame) -> bool {
        let phys_addr = frame.start_address().as_u64();
        if phys_addr & 0xfff != 0 || !self.is_usable_frame_addr(phys_addr) {
            return false;
        }
        if self.recycled_count >= RECYCLED_FRAME_CAP {
            return false;
        }
        // double-free 防止
        for i in 0..self.recycled_count {
            if self.recycled_frames[i] == phys_addr {
                return false;
            }
        }
        self.recycled_frames[self.recycled_count] = phys_addr;
        self.recycled_count += 1;
        true
    }

    /// 使用可能な物理メモリの総量を計算（バイト）
    ///
    /// ## Returns
    /// 使用可能な物理メモリの総量（バイト）
    pub fn usable_memory(&self) -> u64 {
        self.memory_map
            .iter()
            .filter(|r| r.region_type == MemoryType::Usable)
            .map(|r| r.len)
            .sum()
    }

    /// 使用可能なフレーム数を計算
    ///
    /// ## Returns
    /// 使用可能なフレーム数
    pub fn usable_frames(&self) -> usize {
        (self.usable_memory() / 4096) as usize
    }

    /// 使用可能なフレームのイテレータを返す
    ///
    /// ## Returns
    /// 使用可能なフレームのイテレータ
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
    /// フレームを割り当て
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        if self.recycled_count > 0 {
            self.recycled_count -= 1;
            let phys = self.recycled_frames[self.recycled_count];
            return Some(PhysFrame::containing_address(PhysAddr::new(phys)));
        }

        let mut f = self.next_frame as u64;
        let max_frame = self
            .memory_map
            .iter()
            .map(|r| (r.start + r.len) / 4096)
            .max()
            .unwrap_or(0);

        while f <= max_frame {
            let phys_addr = f * 4096;
            let mut usable = false;

            for r in self.memory_map.iter() {
                if r.region_type != MemoryType::Usable {
                    continue;
                }
                if phys_addr >= r.start && phys_addr < r.start + r.len {
                    usable = true;
                    break;
                }
            }

            if usable {
                self.next_frame = (f + 1) as usize;
                return Some(PhysFrame::containing_address(PhysAddr::new(phys_addr)));
            }
            f += 1;
        }
        None
    }
}

/// フレームアロケータを初期化
///
/// ## Arguments
/// - `memory_map`: ブートローダーから提供されたメモリマップ
pub fn init(memory_map: &'static [MemoryRegion]) {
    let allocator = BitmapFrameAllocator::new(memory_map);
    *FRAME_ALLOCATOR.lock() = Some(allocator);
}

/// フレームを割り当て
///
/// ## Returns
/// 割り当てられたフレーム、またはエラー
pub fn allocate_frame() -> Result<PhysFrame> {
    FRAME_ALLOCATOR
        .lock()
        .as_mut()
        .and_then(|a| a.allocate_frame())
        .ok_or(Kernel::Memory(Memory::OutOfMemory))
}

/// フレームを解放
pub fn deallocate_frame(frame: PhysFrame) -> Result<()> {
    let mut guard = FRAME_ALLOCATOR.lock();
    let allocator = guard.as_mut().ok_or(Kernel::Memory(Memory::OutOfMemory))?;
    if allocator.deallocate_frame(frame) {
        Ok(())
    } else {
        Err(Kernel::Memory(Memory::InvalidAddress))
    }
}

/// 使用可能なメモリ情報を取得
///
/// ## Returns
/// 使用可能な物理メモリの総量（バイト）と使用可能
pub fn get_memory_info() -> Option<(u64, usize)> {
    FRAME_ALLOCATOR
        .lock()
        .as_ref()
        .map(|a| (a.usable_memory(), a.usable_frames()))
}
