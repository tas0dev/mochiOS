//! mochiOS app-side window helper.

#![cfg(all(target_os = "linux", target_env = "musl"))]

use swiftlib::{
    ipc::{ipc_recv, ipc_send},
    privileged,
    task::{find_process_by_name, yield_now},
};

use crate::ipc_proto::{
    IPC_BUF_SIZE, OP_REQ_ATTACH_SHARED, OP_REQ_CREATE_WINDOW, OP_REQ_PRESENT_SHARED,
    OP_RES_SHARED_ATTACHED, OP_RES_WINDOW_CREATED,
};

struct SharedSurface {
    virt_addr: u64,
    total_pixels: usize,
}

pub struct Window {
    kagami_tid: u64,
    window_id: u32,
    width: u16,
    height: u16,
    surface: SharedSurface,
    #[allow(dead_code)]
    phys_pages: Vec<u64>,
}

impl Window {
    pub fn new(width: u16, height: u16, layer: u8) -> Result<Self, &'static str> {
        let kagami_tid = find_kagami_tid().ok_or("Kagami not found")?;
        let window_id = create_window(kagami_tid, width, height, layer)?;

        let total = width as usize * height as usize;
        let total_bytes = total.checked_mul(4).ok_or("size overflow")?;
        let page_count = total_bytes.div_ceil(4096);
        if page_count == 0 {
            return Err("shared surface page count out of range");
        }

        let mut phys_pages = vec![0u64; page_count];
        let virt_addr = unsafe {
            privileged::alloc_shared_pages(page_count as u64, Some(phys_pages.as_mut_slice()), 0)
        };
        if (virt_addr as i64) < 0 || virt_addr == 0 {
            return Err("alloc_shared_pages failed");
        }

        // Attach shared surface once.
        attach_shared(kagami_tid, window_id, width, height, phys_pages.as_slice())?;

        Ok(Self {
            kagami_tid,
            window_id,
            width,
            height,
            surface: SharedSurface {
                virt_addr,
                total_pixels: total,
            },
            phys_pages,
        })
    }

    pub fn id(&self) -> u32 {
        self.window_id
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub fn present(&mut self, pixels: &[u32]) -> Result<(), &'static str> {
        if pixels.len() < self.surface.total_pixels {
            return Err("pixel buffer too small");
        }
        blit_shared_surface(&self.surface, pixels);
        // Kagami 側の描画更新タイミング次第で 1 回だと反映されないことがあるので、
        // attach 直後や起動直後は複数回 present する。
        for _ in 0..3 {
            present_shared(self.kagami_tid, self.window_id)?;
            yield_now();
        }
        Ok(())
    }
}

fn find_kagami_tid() -> Option<u64> {
    // Binder などから `--kagami-tid=<tid>` が渡されるので最優先で使う。
    if let Some(tid) = parse_kagami_tid_from_args() {
        return Some(tid);
    }

    // 起動経路や bundle 名でプロセス名が揺れても動くように複数候補を試す。
    for name in ["/applications/Kagami.app/entry.elf", "Kagami.app", "entry.elf"] {
        if let Some(tid) = find_process_by_name(name) {
            return Some(tid);
        }
    }
    None
}

fn parse_kagami_tid_from_args() -> Option<u64> {
    for arg in std::env::args().skip(1) {
        if let Some(rest) = arg.strip_prefix("--kagami-tid=")
            && let Ok(tid) = rest.parse::<u64>()
            && tid != 0
        {
            return Some(tid);
        }
    }
    None
}

fn create_window(kagami_tid: u64, width: u16, height: u16, layer: u8) -> Result<u32, &'static str> {
    let mut req = [0u8; 9];
    req[0..4].copy_from_slice(&OP_REQ_CREATE_WINDOW.to_le_bytes());
    req[4..6].copy_from_slice(&width.to_le_bytes());
    req[6..8].copy_from_slice(&height.to_le_bytes());
    req[8] = layer;
    if (ipc_send(kagami_tid, &req) as i64) < 0 {
        return Err("send create window failed");
    }
    let mut recv = [0u8; IPC_BUF_SIZE];
    for _ in 0..256 {
        let (sender, len) = ipc_recv(&mut recv);
        if sender != kagami_tid || len < 8 {
            yield_now();
            continue;
        }
        let op = u32::from_le_bytes([recv[0], recv[1], recv[2], recv[3]]);
        if op != OP_RES_WINDOW_CREATED {
            continue;
        }
        return Ok(u32::from_le_bytes([recv[4], recv[5], recv[6], recv[7]]));
    }
    Err("window create timeout")
}

fn attach_shared(
    kagami_tid: u64,
    window_id: u32,
    width: u16,
    height: u16,
    phys_pages: &[u64],
) -> Result<(), &'static str> {
    let mut attach = [0u8; 12];
    attach[0..4].copy_from_slice(&OP_REQ_ATTACH_SHARED.to_le_bytes());
    attach[4..8].copy_from_slice(&window_id.to_le_bytes());
    attach[8..10].copy_from_slice(&width.to_le_bytes());
    attach[10..12].copy_from_slice(&height.to_le_bytes());
    if (ipc_send(kagami_tid, &attach) as i64) < 0 {
        return Err("failed to send shared attach");
    }
    let send_pages_ret = unsafe { privileged::ipc_send_pages(kagami_tid, phys_pages, 0) };
    if (send_pages_ret as i64) < 0 {
        return Err("failed to send shared pages");
    }
    wait_shared_attach_ack(kagami_tid, window_id)
}

fn wait_shared_attach_ack(kagami_tid: u64, window_id: u32) -> Result<(), &'static str> {
    let mut recv = [0u8; IPC_BUF_SIZE];
    for _ in 0..256 {
        let (sender, len) = ipc_recv(&mut recv);
        if sender != kagami_tid || len < 8 {
            yield_now();
            continue;
        }
        let op = u32::from_le_bytes([recv[0], recv[1], recv[2], recv[3]]);
        if op != OP_RES_SHARED_ATTACHED {
            continue;
        }
        let res_window = u32::from_le_bytes([recv[4], recv[5], recv[6], recv[7]]);
        if res_window == window_id {
            return Ok(());
        }
    }
    Err("shared attach timeout")
}

fn present_shared(kagami_tid: u64, window_id: u32) -> Result<(), &'static str> {
    let mut req = [0u8; 8];
    req[0..4].copy_from_slice(&OP_REQ_PRESENT_SHARED.to_le_bytes());
    req[4..8].copy_from_slice(&window_id.to_le_bytes());
    if (ipc_send(kagami_tid, &req) as i64) < 0 {
        return Err("send present shared failed");
    }
    Ok(())
}

fn blit_shared_surface(surface: &SharedSurface, pixels: &[u32]) {
    unsafe {
        let ptr = surface.virt_addr as *mut u32;
        let slice = core::slice::from_raw_parts_mut(ptr, surface.total_pixels);
        slice.copy_from_slice(&pixels[..surface.total_pixels]);
    }
}
