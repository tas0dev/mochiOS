use swiftlib::{ipc, keyboard, mouse, privileged, process, task};

use crate::input::InputState;
use crate::ipc_proto::{
    IPC_BUF_SIZE, LAYER_APP, LAYER_STATUS, LAYER_SYSTEM, LAYER_WALLPAPER, OP_REQ_ATTACH_SHARED,
    OP_REQ_CREATE_WINDOW, OP_REQ_FLUSH, OP_REQ_FLUSH_CHUNK, OP_REQ_PRESENT_SHARED,
    OP_RES_SHARED_ATTACHED, OP_RES_WINDOW_CREATED,
};
use crate::renderer::{Renderer, WindowLayer};

const FLUSH_FULL_HEADER_SIZE: usize = 12;
const FLUSH_CHUNK_HEADER_SIZE: usize = 20;
const IPC_MAX_PIXELS_FULL: usize = (IPC_BUF_SIZE - FLUSH_FULL_HEADER_SIZE) / 4;
const IPC_MAX_PIXELS_CHUNK: usize = (IPC_BUF_SIZE - FLUSH_CHUNK_HEADER_SIZE) / 4;
const MOUSE_BURST_LIMIT: usize = 8;
const DOCK_PROCESS_CANDIDATES: [&str; 2] = ["/applications/Dock.app/entry.elf", "Dock.app"];

#[derive(Clone, Copy)]
struct DragState {
    window_id: u32,
    grab_dx: i32,
    grab_dy: i32,
}

#[derive(Clone, Copy)]
struct PendingSharedAttach {
    sender_tid: u64,
    window_id: u32,
    width: usize,
    height: usize,
}

pub struct KagamiApp {
    renderer: Renderer,
    input: InputState,
    warned_mouse_err: bool,
    ipc_buf: [u8; IPC_BUF_SIZE],
    next_window_id: u32,
    demo_windows_created: bool,
    secure_input_mode: bool,
    prev_left_down: bool,
    drag_state: Option<DragState>,
    viewkit_key_down: bool,
    binder_key_down: bool,
    dock_key_down: bool,
    terminal_key_down: bool,
    pending_shared_attach: Option<PendingSharedAttach>,
}

impl KagamiApp {
    pub fn new(renderer: Renderer) -> Self {
        Self {
            renderer,
            input: InputState::new(),
            warned_mouse_err: false,
            ipc_buf: [0u8; IPC_BUF_SIZE],
            next_window_id: 1,
            demo_windows_created: false,
            secure_input_mode: false,
            prev_left_down: false,
            drag_state: None,
            viewkit_key_down: false,
            binder_key_down: false,
            dock_key_down: false,
            terminal_key_down: false,
            pending_shared_attach: None,
        }
    }

    pub fn run(&mut self) {
        self.renderer.initialize();
        println!(
            "[KAGAMI] started (ESC to exit, D demo, V ViewKit, B Binder, O Dock, T Terminal) tid={}",
            task::gettid()
        );
        self.launch_binder();

        loop {
            let mut did_work = false;

            if self.process_mouse_events_prioritized() {
                did_work = true;
            }

            let sc_opt = match keyboard::read_scancode_tap() {
                Ok(Some(sc)) => Some(sc),
                Ok(None) => keyboard::read_scancode(),
                Err(_) => keyboard::read_scancode(),
            };

            if let Some(sc) = sc_opt {
                did_work = true;
                if self.input.should_exit(sc) {
                    println!("[KAGAMI] exit");
                    return;
                }
                if sc == 0x20 || sc == 0xA0 {
                    self.inject_demo_ipc();
                }
                if sc == 0x2F && !self.viewkit_key_down {
                    self.viewkit_key_down = true;
                    self.launch_viewkit_ui_test();
                }
                if sc == 0xAF {
                    self.viewkit_key_down = false;
                }
                if sc == 0x30 && !self.binder_key_down {
                    self.binder_key_down = true;
                    self.launch_binder();
                }
                if sc == 0xB0 {
                    self.binder_key_down = false;
                }
                if sc == 0x18 && !self.dock_key_down {
                    self.dock_key_down = true;
                    self.launch_dock();
                }
                if sc == 0x98 {
                    self.dock_key_down = false;
                }
                if sc == 0x14 && !self.terminal_key_down {
                    self.terminal_key_down = true;
                    self.launch_terminal();
                }
                if sc == 0x94 {
                    self.terminal_key_down = false;
                }
            }

            if self.process_ipc_messages() {
                did_work = true;
            }

            self.update_secure_input_mode();
            if self.renderer.tick_animations() {
                did_work = true;
            }

            if !did_work {
                task::yield_now();
            }
        }
    }

    fn process_mouse_events_prioritized(&mut self) -> bool {
        let mut handled = false;
        let mut sum_dx = 0i32;
        let mut sum_dy = 0i32;
        let mut latest_left: Option<bool> = None;
        for _ in 0..MOUSE_BURST_LIMIT {
            match mouse::read_packet_raw() {
                Ok(Some(packet)) => {
                    handled = true;
                    if let Some((dx, dy)) = self.input.consume_mouse(packet) {
                        sum_dx = sum_dx.saturating_add(dx);
                        sum_dy = sum_dy.saturating_add(dy);
                    }
                    latest_left = Some(packet.left());
                }
                Ok(None) => break,
                Err(err) => {
                    if !self.warned_mouse_err {
                        eprintln!("[KAGAMI] mouse read error: {}", err as i64);
                        self.warned_mouse_err = true;
                    }
                    break;
                }
            }
        }
        if handled {
            if sum_dx != 0 || sum_dy != 0 {
                self.renderer.move_cursor_by(sum_dx, sum_dy);
            }
            if let Some(left) = latest_left {
                self.handle_pointer_buttons(left);
            }
        }
        handled
    }

    fn process_ipc_messages(&mut self) -> bool {
        let mut handled = false;
        loop {
            let (sender, len) = ipc::ipc_recv(&mut self.ipc_buf);
            if sender == 0 || len == 0 {
                break;
            }
            let len = (len as usize).min(self.ipc_buf.len());
            self.handle_ipc_message(sender, len);
            handled = true;
        }
        handled
    }

    fn handle_ipc_message(&mut self, sender_tid: u64, len: usize) {
        if let Some(pending) = self.pending_shared_attach {
            const MAP_HEADER_MAGIC: u32 = 0xABCD_DCBAu32;
            if pending.sender_tid == sender_tid && len >= 20 {
                let magic = u32::from_le_bytes([
                    self.ipc_buf[0],
                    self.ipc_buf[1],
                    self.ipc_buf[2],
                    self.ipc_buf[3],
                ]);
                if magic == MAP_HEADER_MAGIC {
                    let mapped_addr = u64::from_le_bytes([
                        self.ipc_buf[4],
                        self.ipc_buf[5],
                        self.ipc_buf[6],
                        self.ipc_buf[7],
                        self.ipc_buf[8],
                        self.ipc_buf[9],
                        self.ipc_buf[10],
                        self.ipc_buf[11],
                    ]);
                    let total_bytes = u64::from_le_bytes([
                        self.ipc_buf[12],
                        self.ipc_buf[13],
                        self.ipc_buf[14],
                        self.ipc_buf[15],
                        self.ipc_buf[16],
                        self.ipc_buf[17],
                        self.ipc_buf[18],
                        self.ipc_buf[19],
                    ]);
                    println!("[KAGAMI] received map header from {} mapped=0x{:x} total_bytes={}", sender_tid, mapped_addr, total_bytes);
                    if self.renderer.attach_mapped_shared_surface(
                        pending.window_id,
                        pending.width,
                        pending.height,
                        mapped_addr,
                        total_bytes,
                    ) {
                        println!("[KAGAMI] attach_mapped_shared_surface OK for window {}", pending.window_id);
                        self.pending_shared_attach = None;
                        let mut res = [0u8; 8];
                        res[0..4].copy_from_slice(&OP_RES_SHARED_ATTACHED.to_le_bytes());
                        res[4..8].copy_from_slice(&pending.window_id.to_le_bytes());
                        let _ = ipc::ipc_send(sender_tid, &res);
                        println!("[KAGAMI] sent OP_RES_SHARED_ATTACHED to {} for window {}", sender_tid, pending.window_id);
                        return;
                    } else {
                        println!("[KAGAMI] attach_mapped_shared_surface failed for window {}", pending.window_id);
                    }
                }
            }
        }
        if len < 4 {
            return;
        }
        let op = u32::from_le_bytes([
            self.ipc_buf[0],
            self.ipc_buf[1],
            self.ipc_buf[2],
            self.ipc_buf[3],
        ]);
        match op {
            OP_REQ_CREATE_WINDOW => {
                if len < 8 {
                    return;
                }
                let req_w = u16::from_le_bytes([self.ipc_buf[4], self.ipc_buf[5]]) as usize;
                let req_h = u16::from_le_bytes([self.ipc_buf[6], self.ipc_buf[7]]) as usize;
                let mut requested_layer = if len >= 9 { self.ipc_buf[8] } else { LAYER_APP };
                let width = req_w.clamp(8, 1024);
                let height = req_h.clamp(8, 1024);
                let privilege = task::get_thread_privilege(sender_tid);
                if is_sender_dock(sender_tid) {
                    requested_layer = LAYER_STATUS;
                }
                if privilege <= 1 && width <= 400 && height <= 140 {
                    requested_layer = LAYER_STATUS;
                }
                let layer = sanitize_layer_request(requested_layer, privilege);
                let window_id = self.next_window_id;
                self.next_window_id = self.next_window_id.saturating_add(1);
                let init_color = if layer == WindowLayer::Status { 0x0000_0000 } else { 0xFF30_3048 };
                self.renderer.create_window(
                    window_id,
                    layer,
                    width,
                    height,
                    vec![init_color; width * height],
                );
                let mut res = [0u8; 8];
                res[0..4].copy_from_slice(&OP_RES_WINDOW_CREATED.to_le_bytes());
                res[4..8].copy_from_slice(&window_id.to_le_bytes());
                let _ = ipc::ipc_send(sender_tid, &res);
            }
            OP_REQ_FLUSH => {
                if len < 12 {
                    return;
                }
                let window_id = u32::from_le_bytes([
                    self.ipc_buf[4],
                    self.ipc_buf[5],
                    self.ipc_buf[6],
                    self.ipc_buf[7],
                ]);
                let width = u16::from_le_bytes([self.ipc_buf[8], self.ipc_buf[9]]) as usize;
                let height = u16::from_le_bytes([self.ipc_buf[10], self.ipc_buf[11]]) as usize;
                let pixel_count = width.saturating_mul(height);
                let needed = 12usize.saturating_add(pixel_count.saturating_mul(4));
                if width == 0 || height == 0 || pixel_count > IPC_MAX_PIXELS_FULL || len < needed {
                    return;
                }
                let mut pixels = Vec::with_capacity(pixel_count);
                let mut off = 12usize;
                for _ in 0..pixel_count {
                    let px = u32::from_le_bytes([
                        self.ipc_buf[off],
                        self.ipc_buf[off + 1],
                        self.ipc_buf[off + 2],
                        self.ipc_buf[off + 3],
                    ]);
                    pixels.push(normalize_alpha(px));
                    off += 4;
                }
                self.renderer
                    .update_window_pixels(window_id, width, height, pixels);
            }
            OP_REQ_FLUSH_CHUNK => {
                if len < FLUSH_CHUNK_HEADER_SIZE {
                    return;
                }
                let window_id = u32::from_le_bytes([
                    self.ipc_buf[4],
                    self.ipc_buf[5],
                    self.ipc_buf[6],
                    self.ipc_buf[7],
                ]);
                let width = u16::from_le_bytes([self.ipc_buf[8], self.ipc_buf[9]]) as usize;
                let height = u16::from_le_bytes([self.ipc_buf[10], self.ipc_buf[11]]) as usize;
                let chunk_x = u16::from_le_bytes([self.ipc_buf[12], self.ipc_buf[13]]) as usize;
                let chunk_y = u16::from_le_bytes([self.ipc_buf[14], self.ipc_buf[15]]) as usize;
                let chunk_w = u16::from_le_bytes([self.ipc_buf[16], self.ipc_buf[17]]) as usize;
                let chunk_h = u16::from_le_bytes([self.ipc_buf[18], self.ipc_buf[19]]) as usize;
                let pixel_count = chunk_w.saturating_mul(chunk_h);
                let needed = FLUSH_CHUNK_HEADER_SIZE.saturating_add(pixel_count.saturating_mul(4));
                if width == 0
                    || height == 0
                    || chunk_w == 0
                    || chunk_h == 0
                    || pixel_count > IPC_MAX_PIXELS_CHUNK
                    || chunk_x.saturating_add(chunk_w) > width
                    || chunk_y.saturating_add(chunk_h) > height
                    || len < needed
                {
                    return;
                }
                let mut pixels = Vec::with_capacity(pixel_count);
                let mut off = FLUSH_CHUNK_HEADER_SIZE;
                for _ in 0..pixel_count {
                    let px = u32::from_le_bytes([
                        self.ipc_buf[off],
                        self.ipc_buf[off + 1],
                        self.ipc_buf[off + 2],
                        self.ipc_buf[off + 3],
                    ]);
                    pixels.push(normalize_alpha(px));
                    off += 4;
                }
                self.renderer.update_window_chunk_pixels(
                    window_id, width, height, chunk_x, chunk_y, chunk_w, chunk_h, &pixels,
                );
            }
            OP_REQ_ATTACH_SHARED => {
                println!("[KAGAMI] OP_REQ_ATTACH_SHARED received from {} (len={})", sender_tid, len);
                if len < 12 {
                    println!("[KAGAMI] OP_REQ_ATTACH_SHARED: message too short (len={})", len);
                    return;
                }
                let window_id = u32::from_le_bytes([
                    self.ipc_buf[4],
                    self.ipc_buf[5],
                    self.ipc_buf[6],
                    self.ipc_buf[7],
                ]);
                let width = u16::from_le_bytes([self.ipc_buf[8], self.ipc_buf[9]]) as usize;
                let height = u16::from_le_bytes([self.ipc_buf[10], self.ipc_buf[11]]) as usize;
                if width == 0 || height == 0 {
                    println!("[KAGAMI] OP_REQ_ATTACH_SHARED: invalid size {}x{}", width, height);
                    return;
                }
                let sender_priv = task::get_thread_privilege(sender_tid);
                println!("[KAGAMI] OP_REQ_ATTACH_SHARED: sender_priv={}, window={}, {}x{}", sender_priv, window_id, width, height);
                if sender_priv <= 1 {
                    // Service/Core 側は従来どおり「送信元がページを用意して送る」方式
                    self.pending_shared_attach = Some(PendingSharedAttach {
                        sender_tid,
                        window_id,
                        width,
                        height,
                    });
                    println!("[KAGAMI] pending_shared_attach set for sender {} window {}", sender_tid, window_id);
                } else {
                    // User 側は Kagami が共有面を確保し、ipc_send_pages でユーザーへ配布する
                    let total_bytes = match width.checked_mul(height).and_then(|v| v.checked_mul(4)) {
                        Some(v) => v,
                        None => return,
                    };
                    let page_count = total_bytes.div_ceil(4096);
                    if page_count == 0 {
                        return;
                    }
                    let mut phys_pages = vec![0u64; page_count];
                    println!("[KAGAMI] allocating {} pages for user-shared surface", page_count);
                    let mapped = unsafe {
                        privileged::alloc_shared_pages(page_count as u64, Some(phys_pages.as_mut_slice()), 0)
                    };
                    println!("[KAGAMI] alloc_shared_pages returned mapped=0x{:x}", mapped);
                    if (mapped as i64) < 0 || mapped == 0 {
                        println!("[KAGAMI] alloc_shared_pages failed");
                        return;
                    }
                    if !self.renderer.attach_mapped_shared_surface(
                        window_id,
                        width,
                        height,
                        mapped,
                        (page_count * 4096) as u64,
                    ) {
                        println!("[KAGAMI] attach_mapped_shared_surface failed");
                        return;
                    }
                    println!("[KAGAMI] calling ipc_send_pages to sender {}", sender_tid);
                    let send_ret = unsafe { privileged::ipc_send_pages(sender_tid, phys_pages.as_slice(), 0) };
                    println!("[KAGAMI] ipc_send_pages returned {}", send_ret);
                    if (send_ret as i64) < 0 {
                        println!("[KAGAMI] ipc_send_pages failed: {}", send_ret);
                        return;
                    }
                    let mut res = [0u8; 8];
                    res[0..4].copy_from_slice(&OP_RES_SHARED_ATTACHED.to_le_bytes());
                    res[4..8].copy_from_slice(&window_id.to_le_bytes());
                    let _ = ipc::ipc_send(sender_tid, &res);
                }
            }
            OP_REQ_PRESENT_SHARED => {
                if len < 8 {
                    return;
                }
                let window_id = u32::from_le_bytes([
                    self.ipc_buf[4],
                    self.ipc_buf[5],
                    self.ipc_buf[6],
                    self.ipc_buf[7],
                ]);
                self.renderer.present_shared_surface(window_id);
            }
            _ => {}
        }
    }

    fn update_secure_input_mode(&mut self) {
        let focused_layer = self.renderer.top_layer();
        let next_secure = matches!(focused_layer, Some(WindowLayer::System));
        if next_secure != self.secure_input_mode {
            self.secure_input_mode = next_secure;
            if self.secure_input_mode {
                println!("[KAGAMI] secure input mode: ON");
            } else {
                println!("[KAGAMI] secure input mode: OFF");
            }
        }
    }

    fn handle_pointer_buttons(&mut self, left_down: bool) {
        let (cx, cy) = self.renderer.cursor_pos();
        if !self.prev_left_down && left_down {
            if let Some(window_id) = self.renderer.hit_test_top_window(cx, cy) {
                self.renderer.bring_to_front(window_id);
                if self.renderer.is_title_bar_hit(window_id, cx, cy)
                    && let Some((wx, wy)) = self.renderer.window_pos(window_id)
                {
                    self.drag_state = Some(DragState {
                        window_id,
                        grab_dx: cx - wx,
                        grab_dy: cy - wy,
                    });
                }
            }
        } else if self.prev_left_down && !left_down {
            self.drag_state = None;
        } else if left_down && let Some(drag) = self.drag_state {
            self.renderer
                .move_window_to(drag.window_id, cx - drag.grab_dx, cy - drag.grab_dy);
        }
        self.prev_left_down = left_down;
    }

    fn inject_demo_ipc(&mut self) {
        let self_tid = task::gettid();
        let width_a: u16 = 120;
        let height_a: u16 = 80;
        let width_b: u16 = 104;
        let height_b: u16 = 72;

        if !self.demo_windows_created {
            let mut create_a = [0u8; 9];
            create_a[0..4].copy_from_slice(&OP_REQ_CREATE_WINDOW.to_le_bytes());
            create_a[4..6].copy_from_slice(&width_a.to_le_bytes());
            create_a[6..8].copy_from_slice(&height_a.to_le_bytes());
            create_a[8] = LAYER_APP;
            let _ = ipc::ipc_send(self_tid, &create_a);

            let mut create_b = [0u8; 9];
            create_b[0..4].copy_from_slice(&OP_REQ_CREATE_WINDOW.to_le_bytes());
            create_b[4..6].copy_from_slice(&width_b.to_le_bytes());
            create_b[6..8].copy_from_slice(&height_b.to_le_bytes());
            create_b[8] = LAYER_APP;
            let _ = ipc::ipc_send(self_tid, &create_b);
            self.demo_windows_created = true;
        }

        self.send_checkerboard_chunked(self_tid, 1, width_a as usize, height_a as usize, 0x0066_CCFF, 0x0022_3344);
        self.send_checkerboard_chunked(self_tid, 2, width_b as usize, height_b as usize, 0x00FF_8866, 0x0055_2233);
    }

    fn send_checkerboard_chunked(
        &self,
        target_tid: u64,
        window_id: u32,
        width: usize,
        height: usize,
        c0: u32,
        c1: u32,
    ) {
        let max_chunk_pixels = IPC_MAX_PIXELS_CHUNK.max(1);
        let chunk_w = width.min(64).max(1);
        let chunk_h = (max_chunk_pixels / chunk_w).max(1);
        let mut y0 = 0usize;
        while y0 < height {
            let h = (height - y0).min(chunk_h);
            let mut x0 = 0usize;
            while x0 < width {
                let w = (width - x0).min(chunk_w);
                let mut msg = vec![0u8; FLUSH_CHUNK_HEADER_SIZE + (w * h * 4)];
                msg[0..4].copy_from_slice(&OP_REQ_FLUSH_CHUNK.to_le_bytes());
                msg[4..8].copy_from_slice(&window_id.to_le_bytes());
                msg[8..10].copy_from_slice(&(width as u16).to_le_bytes());
                msg[10..12].copy_from_slice(&(height as u16).to_le_bytes());
                msg[12..14].copy_from_slice(&(x0 as u16).to_le_bytes());
                msg[14..16].copy_from_slice(&(y0 as u16).to_le_bytes());
                msg[16..18].copy_from_slice(&(w as u16).to_le_bytes());
                msg[18..20].copy_from_slice(&(h as u16).to_le_bytes());
                let mut off = FLUSH_CHUNK_HEADER_SIZE;
                for y in 0..h {
                    for x in 0..w {
                        let checker = (((x0 + x) / 2) + ((y0 + y) / 2)) & 1;
                        let c: u32 = if checker == 0 { c0 } else { c1 };
                        msg[off..off + 4].copy_from_slice(&(c | 0xFF00_0000).to_le_bytes());
                        off += 4;
                    }
                }
                let _ = ipc::ipc_send(target_tid, &msg);
                x0 += w;
            }
            y0 += h;
        }
    }

    fn launch_viewkit_ui_test(&self) {
        let kagami_tid = task::gettid();
        let arg_tid = format!("--kagami-tid={}", kagami_tid);
        let args = [arg_tid.as_str()];
        match process::exec_with_args("/applications/ViewKit.app/entry.elf", &args) {
            Ok(pid) => println!("[KAGAMI] launched ViewKit ui_test pid={}", pid),
            Err(_) => eprintln!("[KAGAMI] failed to launch ViewKit ui_test"),
        }
    }

    fn launch_binder(&self) {
        let kagami_tid = task::gettid();
        let arg_tid = format!("--kagami-tid={}", kagami_tid);
        let args = [arg_tid.as_str()];
        match process::exec_with_args("/applications/Binder.app/entry.elf", &args) {
            Ok(pid) => println!("[KAGAMI] launched Binder pid={}", pid),
            Err(_) => eprintln!("[KAGAMI] failed to launch Binder"),
        }
    }

    fn launch_dock(&self) {
        let kagami_tid = task::gettid();
        let arg_tid = format!("--kagami-tid={}", kagami_tid);
        let args = [arg_tid.as_str()];
        match process::exec_with_args("/applications/Dock.app/entry.elf", &args) {
            Ok(pid) => println!("[KAGAMI] launched Dock pid={}", pid),
            Err(_) => eprintln!("[KAGAMI] failed to launch Dock"),
        }
    }

    fn launch_terminal(&self) {
        let kagami_tid = task::gettid();
        let arg_tid = format!("--kagami-tid={}", kagami_tid);
        let args = [arg_tid.as_str()];
        match process::exec_with_args("/applications/Terminal.app/entry.elf", &args) {
            Ok(pid) => println!("[KAGAMI] launched Terminal pid={}", pid),
            Err(_) => eprintln!("[KAGAMI] failed to launch Terminal"),
        }
    }
}

fn sanitize_layer_request(requested: u8, privilege: u64) -> WindowLayer {
    let requested_layer = match requested {
        LAYER_WALLPAPER => WindowLayer::Wallpaper,
        LAYER_STATUS => WindowLayer::Status,
        LAYER_SYSTEM => WindowLayer::System,
        _ => WindowLayer::App,
    };
    let is_privileged = privilege == 0 || privilege == 1;
    if !is_privileged {
        match requested_layer {
            WindowLayer::System => WindowLayer::App,
            other => other,
        }
    } else {
        requested_layer
    }
}

#[inline]
fn normalize_alpha(px: u32) -> u32 {
    if (px & 0xFF00_0000) == 0 && (px & 0x00FF_FFFF) != 0 {
        px | 0xFF00_0000
    } else {
        px
    }
}

fn is_sender_dock(sender_tid: u64) -> bool {
    for name in DOCK_PROCESS_CANDIDATES {
        if let Some(tid) = task::find_process_by_name(name)
            && tid == sender_tid
        {
            return true;
        }
    }
    false
}
