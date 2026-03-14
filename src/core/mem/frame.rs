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

/// グローバルフレームアロケータ
pub static FRAME_ALLOCATOR: Mutex<Option<BitmapFrameAllocator>> = Mutex::new(None);

/// ビットマップベースのフレームアロケータ
///
/// 解放済みフレームはフレーム自身の先頭8バイトにリンクリストのnextポインタを
/// 埋め込むことで上限なしに再利用できる。
pub struct BitmapFrameAllocator {
    /// メモリマップ
    memory_map: &'static [MemoryRegion],
    /// バンプアロケータの次フレームインデックス
    next_frame: usize,
    /// 解放済みフレームのフリーリスト先頭（物理アドレス、0 = 空）
    free_list_head: u64,
    /// HHDM オフセット（phys → virt 変換用）
    phys_offset: u64,
}

impl BitmapFrameAllocator {
    /// 新しいフレームアロケータを作成
    pub fn new(memory_map: &'static [MemoryRegion], phys_offset: u64) -> Self {
        Self {
            memory_map,
            next_frame: 0,
            free_list_head: 0,
            phys_offset,
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
        if self.phys_offset == 0 {
            return false;
        }
        // フレームの先頭8バイトに現在の free_list_head を書き込んでリストに繋ぐ
        let virt_ptr = (phys_addr + self.phys_offset) as *mut u64;
        unsafe { *virt_ptr = self.free_list_head };
        self.free_list_head = phys_addr;
        true
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
        // フリーリストから再利用
        if self.free_list_head != 0 && self.phys_offset != 0 {
            let phys = self.free_list_head;
            let virt_ptr = (phys + self.phys_offset) as *mut u64;
            self.free_list_head = unsafe { *virt_ptr };
            unsafe { *virt_ptr = 0 }; // nextp をゼロクリア
            return Some(PhysFrame::containing_address(PhysAddr::new(phys)));
        }

        // バンプアロケータから新規割り当て
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
pub fn init(memory_map: &'static [MemoryRegion]) {
    let allocator = BitmapFrameAllocator::new(memory_map, 0);
    *FRAME_ALLOCATOR.lock() = Some(allocator);
}

/// ページングが初期化された後に HHDM オフセットをセット
pub fn set_phys_offset(offset: u64) {
    if let Some(alloc) = FRAME_ALLOCATOR.lock().as_mut() {
        alloc.phys_offset = offset;
    }
}

/// フレームを割り当て
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
pub fn get_memory_info() -> Option<(u64, usize)> {
    FRAME_ALLOCATOR
        .lock()
        .as_ref()
        .map(|a| (a.usable_memory(), a.usable_frames()))
}
