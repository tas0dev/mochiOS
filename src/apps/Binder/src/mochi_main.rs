use std::sync::OnceLock;
use swiftlib::{
    ipc::{ipc_recv, ipc_send},
    keyboard::read_scancode_tap,
    privileged,
    process,
    task::{find_process_by_name, yield_now},
    vga,
};
use viewkit::{render_component_to_pixmap, VComponent};

const IPC_BUF_SIZE: usize = 4128;
const KAGAMI_PROCESS_CANDIDATES: [&str; 3] =
    ["/applications/Kagami.app/entry.elf", "Kagami.app", "entry.elf"];

const OP_REQ_CREATE_WINDOW: u32 = 1;
const OP_RES_WINDOW_CREATED: u32 = 2;
const OP_REQ_FLUSH_CHUNK: u32 = 4;
const OP_REQ_ATTACH_SHARED: u32 = 5;
const OP_REQ_PRESENT_SHARED: u32 = 6;
const OP_RES_SHARED_ATTACHED: u32 = 7;
const LAYER_WALLPAPER: u8 = 0;
const FONT_BDF_PATH: &str = "/System/fonts/ter-u12b.bdf";
const FONT_HEIGHT: usize = 12;
const GLYPH_COUNT: usize = 96;
const ASCII_START: usize = 32;
const ASCII_END: usize = ASCII_START + GLYPH_COUNT;

struct Font {
    glyphs: [[u8; FONT_HEIGHT]; GLYPH_COUNT],
}

struct SharedSurface {
    virt_addr: u64,
    page_count: u64,
    total_pixels: usize,
}

struct DesktopWindow {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    title: String,
}

impl Font {
    fn fallback() -> Self {
        let mut glyphs = [[0u8; FONT_HEIGHT]; GLYPH_COUNT];
        for (i, glyph) in glyphs.iter_mut().enumerate() {
            let ch = (ASCII_START + i) as u8;
            if ch == b' ' {
                continue;
            }
            glyph[0] = 0xFC;
            glyph[FONT_HEIGHT - 1] = 0xFC;
            for row in glyph.iter_mut().take(FONT_HEIGHT - 1).skip(1) {
                *row = 0x84;
            }
        }
        Self { glyphs }
    }

    fn load() -> Self {
        let Ok(data) = std::fs::read(FONT_BDF_PATH) else {
            return Self::fallback();
        };
        let mut glyphs = [[0u8; FONT_HEIGHT]; GLYPH_COUNT];
        parse_bdf(&data, &mut glyphs);
        Self { glyphs }
    }

    fn glyph(&self, ch: u8) -> &[u8; FONT_HEIGHT] {
        let idx = if (ASCII_START as u8..ASCII_END as u8).contains(&ch) {
            (ch as usize) - ASCII_START
        } else {
            (b'?' as usize) - ASCII_START
        };
        &self.glyphs[idx]
    }
}

pub fn main() {
    println!("[Binder] start desktop mock");
    let kagami_tid = match parse_kagami_tid_from_args().or_else(find_kagami_tid) {
        Some(tid) => tid,
        None => {
            eprintln!("[Binder] Kagami not found");
            return;
        }
    };

    let (width, height) = desktop_window_size();
    let window_id = match create_app_window(kagami_tid, width, height) {
        Ok(id) => { println!("[Binder] created window id={}", id); id }
        Err(e) => {
            eprintln!("[Binder] create window failed: {}", e);
            return;
        }
    };
    let shared_surface = match setup_shared_surface(kagami_tid, window_id, width, height) {
        Ok(surface) => { println!("[Binder] setup_shared_surface ok"); Some(surface) },
        Err(e) => {
            eprintln!("[Binder] shared setup failed: {}, fallback to chunk", e);
            None
        }
    };

    let mut desktop_windows = Vec::new();
    let pixels = render_desktop(width as usize, height as usize, 0, &desktop_windows);

    let render_res = if let Some(shared) = shared_surface.as_ref() {
        blit_shared_surface(shared, &pixels);
        println!("[Binder] blit_shared_surface done");
        let pres = present_shared(kagami_tid, window_id);
        println!("[Binder] present_shared result: {:?}", pres);
        pres
    } else {
        println!("[Binder] using chunked flush");
        let res = flush_window_chunked(kagami_tid, window_id, width, height, &pixels);
        println!("[Binder] chunked flush result: {:?}", res);
        res
    };
    if let Err(e) = render_res {
        eprintln!("[Binder] render failed: {}", e);
        return;
    }

    println!("[Binder] desktop shown");
    launch_dock(kagami_tid);

    loop {
        let sc_opt = read_scancode_tap().ok().flatten();
        if let Some(sc) = sc_opt {
            if sc == 0x01 {
                println!("[Binder] exit");
                return;
            }
        }
        yield_now();
    }
}

fn redraw_desktop(
    kagami_tid: u64,
    window_id: u32,
    width: u16,
    height: u16,
    shared_surface: Option<&SharedSurface>,
    windows: &[DesktopWindow],
) {
    let pixels = render_desktop(width as usize, height as usize, 0, windows);

    if let Some(shared) = shared_surface {
        blit_shared_surface(shared, &pixels);
        let _ = present_shared(kagami_tid, window_id);
    }
}

fn create_app_window(kagami_tid: u64, width: u16, height: u16) -> Result<u32, &'static str> {
    let mut req = [0u8; 9];
    req[0..4].copy_from_slice(&OP_REQ_CREATE_WINDOW.to_le_bytes());
    req[4..6].copy_from_slice(&width.to_le_bytes());
    req[6..8].copy_from_slice(&height.to_le_bytes());
    req[8] = LAYER_WALLPAPER;
    if (ipc_send(kagami_tid, &req) as i64) < 0 {
        return Err("send create_window failed");
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

fn setup_shared_surface(
    kagami_tid: u64,
    window_id: u32,
    width: u16,
    height: u16,
) -> Result<SharedSurface, &'static str> {
    println!("[Binder] setup_shared_surface: width={} height={}", width, height);
    let total = width as usize * height as usize;
    let total_bytes = total.checked_mul(4).ok_or("size overflow")?;
    let page_count = total_bytes.div_ceil(4096);
    if page_count == 0 {
        return Err("shared surface page count out of range");
    }

    println!("[Binder] setup_shared_surface: requesting {} pages", page_count);
    let mut phys_pages = vec![0u64; page_count];
    let virt_addr = unsafe {
        privileged::alloc_shared_pages(page_count as u64, Some(phys_pages.as_mut_slice()), 0)
    };
    println!("[Binder] alloc_shared_pages -> virt={:#x}", virt_addr);
    if (virt_addr as i64) < 0 || virt_addr == 0 {
        println!("[Binder] alloc_shared_pages failed -> {}", virt_addr as i64);
        return Err("alloc_shared_pages failed");
    }

    // Log physical pages allocated
    println!("[Binder] phys_pages (first 8):");
    for i in 0..(phys_pages.len().min(8)) {
        println!("  [{}] = {:#x}", i, phys_pages[i]);
    }
    let all_zero = phys_pages.iter().all(|&x| x == 0);
    if all_zero {
        println!("[Binder] Warning: phys_pages all zero after alloc_shared_pages");
        return Err("alloc_shared_pages returned zeroed phys pages");
    }

    let mut attach = [0u8; 12];
    attach[0..4].copy_from_slice(&OP_REQ_ATTACH_SHARED.to_le_bytes());
    attach[4..8].copy_from_slice(&window_id.to_le_bytes());
    attach[8..10].copy_from_slice(&width.to_le_bytes());
    attach[10..12].copy_from_slice(&height.to_le_bytes());
    println!("[Binder] sending attach request");
    if (ipc_send(kagami_tid, &attach) as i64) < 0 {
        println!("[Binder] ipc_send attach failed");
        return Err("failed to send shared attach");
    }
    println!("[Binder] ipc_send attach ok");
    println!("[Binder] sending pages to kagami tid={}", kagami_tid);
    let send_pages_ret = unsafe { privileged::ipc_send_pages(kagami_tid, phys_pages.as_slice(), 0) };
    println!("[Binder] ipc_send_pages ret {}", send_pages_ret as i64);
    if (send_pages_ret as i64) < 0 {
        println!("[Binder] ipc_send_pages failed");
        return Err("failed to send shared pages");
    }
    println!("[Binder] waiting for shared attach ack");
    wait_shared_attach_ack(kagami_tid, window_id)?;
    println!("[Binder] shared attach ack received");

    Ok(SharedSurface {
        virt_addr,
        page_count: page_count as u64,
        total_pixels: total,
    })
}

fn present_shared(kagami_tid: u64, window_id: u32) -> Result<(), &'static str> {
    let mut present = [0u8; 8];
    present[0..4].copy_from_slice(&OP_REQ_PRESENT_SHARED.to_le_bytes());
    present[4..8].copy_from_slice(&window_id.to_le_bytes());
    if (ipc_send(kagami_tid, &present) as i64) < 0 {
        return Err("failed to send shared present");
    }
    Ok(())
}

fn blit_shared_surface(surface: &SharedSurface, pixels: &[u32]) {
    let count = surface.total_pixels.min(pixels.len());
    let mapped_pixels = (surface.page_count as usize).saturating_mul(4096) / 4;
    let count = count.min(mapped_pixels);
    unsafe {
        let dst = core::slice::from_raw_parts_mut(surface.virt_addr as *mut u32, count);
        for (d, s) in dst.iter_mut().zip(pixels.iter().take(count)) {
            *d = *s | 0xFF00_0000;
        }
    }
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
        let ack_window = u32::from_le_bytes([recv[4], recv[5], recv[6], recv[7]]);
        if ack_window == window_id {
            return Ok(());
        }
    }
    Err("shared attach ack timeout")
}

fn flush_window_chunked(
    kagami_tid: u64,
    window_id: u32,
    width: u16,
    height: u16,
    pixels: &[u32],
) -> Result<(), &'static str> {
    let total = width as usize * height as usize;
    if pixels.len() < total {
        return Err("pixel buffer too small");
    }
    let chunk_header = 20usize;
    let max_chunk_pixels = (IPC_BUF_SIZE - chunk_header) / 4;
    let width_usize = width as usize;
    let height_usize = height as usize;
    let chunk_w = width_usize.min(96).max(1);
    let chunk_h = (max_chunk_pixels / chunk_w).max(1);

    let mut y0 = 0usize;
    while y0 < height_usize {
        let h = (height_usize - y0).min(chunk_h);
        let mut x0 = 0usize;
        while x0 < width_usize {
            let w = (width_usize - x0).min(chunk_w);
            let mut msg = vec![0u8; chunk_header + (w * h * 4)];
            msg[0..4].copy_from_slice(&OP_REQ_FLUSH_CHUNK.to_le_bytes());
            msg[4..8].copy_from_slice(&window_id.to_le_bytes());
            msg[8..10].copy_from_slice(&width.to_le_bytes());
            msg[10..12].copy_from_slice(&height.to_le_bytes());
            msg[12..14].copy_from_slice(&(x0 as u16).to_le_bytes());
            msg[14..16].copy_from_slice(&(y0 as u16).to_le_bytes());
            msg[16..18].copy_from_slice(&(w as u16).to_le_bytes());
            msg[18..20].copy_from_slice(&(h as u16).to_le_bytes());
            let mut off = chunk_header;
            for row in 0..h {
                let src_row = (y0 + row) * width_usize;
                for col in 0..w {
                    msg[off..off + 4]
                        .copy_from_slice(&(pixels[src_row + x0 + col] | 0xFF00_0000).to_le_bytes());
                    off += 4;
                }
            }
            if (ipc_send(kagami_tid, &msg) as i64) < 0 {
                return Err("send flush chunk failed");
            }
            x0 += w;
        }
        y0 += h;
    }
    Ok(())
}

fn render_desktop(width: usize, height: usize, _dock_offset: i32, windows: &[DesktopWindow]) -> Vec<u32> {
    let mut px: Vec<u32> = (0..height)
        .flat_map(|y| {
            let r = (198 * (height - y) + 137 * y) / height;
            let g = (222 * (height - y) + 180 * y) / height;
            let b = (234 * (height - y) + 204 * y) / height;
            
            let color = 0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32);
            std::iter::repeat(color).take(width)
        })
        .collect();

    for window in windows {
        draw_window(&mut px, width, window);
    }
    draw_info_bar(&mut px, width);
    
    px
}

fn draw_info_bar(px: &mut [u32], stride: usize) {
    const INFO_BAR_HEIGHT: i32 = 28;
    fill_rect(px, stride, 0, 0, stride as i32, INFO_BAR_HEIGHT, 0xFFF4_F7FA);
    fill_rect(px, stride, 0, INFO_BAR_HEIGHT - 1, stride as i32, 1, 0xFFE0_E5EE);
    draw_text(px, stride, 10, 8, "mochiOS", 0xFF3A_3F4B);
}

fn draw_window(px: &mut [u32], stride: usize, window: &DesktopWindow) {
    let pixmap = render_window_pixmap(&window.title, window.width as u32, window.height as u32);
    blit_pixmap(px, stride, window.x, window.y, window.width as usize, window.height as usize, &pixmap);
}

fn fill_rect(px: &mut [u32], stride: usize, x: i32, y: i32, w: i32, h: i32, color: u32) {
    if w <= 0 || h <= 0 {
        return;
    }
    let hmax = px.len() / stride;
    let x0 = x.max(0) as usize;
    let y0 = y.max(0) as usize;
    let x1 = (x + w).max(0) as usize;
    let y1 = (y + h).max(0) as usize;
    let x1 = x1.min(stride);
    let y1 = y1.min(hmax);
    for yy in y0..y1 {
        let row = yy * stride;
        for xx in x0..x1 {
            px[row + xx] = color;
        }
    }
}

fn fill_rounded_rect(
    px: &mut [u32],
    stride: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
    color: u32,
) {
    if w <= 0 || h <= 0 {
        return;
    }
    let r = radius.min(w / 2).min(h / 2).max(0);
    for yy in 0..h {
        for xx in 0..w {
            let cov = rounded_rect_coverage(xx, yy, w, h, r);
            if cov != 0 {
                blend_put(px, stride, x + xx, y + yy, color, cov);
            }
        }
    }
}

fn stroke_rounded_rect(
    px: &mut [u32],
    stride: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
    color: u32,
) {
    if w <= 2 || h <= 2 {
        return;
    }
    let r = radius.min(w / 2).min(h / 2).max(0);
    for yy in 0..h {
        for xx in 0..w {
            let outer = rounded_rect_coverage(xx, yy, w, h, r);
            let inner = rounded_rect_coverage(xx - 1, yy - 1, w - 2, h - 2, (r - 1).max(0));
            let cov = outer.saturating_sub(inner);
            if cov != 0 {
                blend_put(px, stride, x + xx, y + yy, color, cov);
            }
        }
    }
}

fn rounded_rect_coverage(xx: i32, yy: i32, w: i32, h: i32, r: i32) -> u8 {
    if w <= 0 || h <= 0 || xx < 0 || yy < 0 || xx >= w || yy >= h {
        return 0;
    }
    let samples = [
        (0.25f32, 0.25f32),
        (0.75f32, 0.25f32),
        (0.25f32, 0.75f32),
        (0.75f32, 0.75f32),
    ];
    let mut hit = 0u8;
    for (ox, oy) in samples {
        if inside_rounded_rect_f(xx as f32 + ox, yy as f32 + oy, w as f32, h as f32, r as f32) {
            hit += 1;
        }
    }
    hit.saturating_mul(64)
}

fn inside_rounded_rect_f(x: f32, y: f32, w: f32, h: f32, r: f32) -> bool {
    if x < 0.0 || y < 0.0 || x >= w || y >= h {
        return false;
    }
    if r <= 0.0 || (x >= r && x < w - r) || (y >= r && y < h - r) {
        return true;
    }
    let cx = if x < r { r } else { w - r };
    let cy = if y < r { r } else { h - r };
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= r * r
}

fn blend_put(px: &mut [u32], stride: usize, x: i32, y: i32, src: u32, alpha: u8) {
    if x < 0 || y < 0 || alpha == 0 {
        return;
    }
    let x = x as usize;
    let y = y as usize;
    let h = px.len() / stride;
    if x >= stride || y >= h {
        return;
    }
    let idx = y * stride + x;
    let dst = px[idx];
    px[idx] = blend_rgb(dst, src, alpha);
}

fn blend_rgb(dst: u32, src: u32, alpha: u8) -> u32 {
    if alpha == 255 {
        return src | 0xFF00_0000;
    }
    let a = alpha as u32;
    let inv = 255u32.saturating_sub(a);
    let sr = (src >> 16) & 0xFF;
    let sg = (src >> 8) & 0xFF;
    let sb = src & 0xFF;
    let dr = (dst >> 16) & 0xFF;
    let dg = (dst >> 8) & 0xFF;
    let db = dst & 0xFF;
    let r = (sr * a + dr * inv) / 255;
    let g = (sg * a + dg * inv) / 255;
    let b = (sb * a + db * inv) / 255;
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

fn draw_text(px: &mut [u32], stride: usize, x: i32, y: i32, text: &str, color: u32) {
    let font = binder_font();
    let mut pen_x = x;
    for ch in text.bytes() {
        draw_char(px, stride, pen_x, y, ch, color, font);
        pen_x += 9;
    }
}

fn draw_char(px: &mut [u32], stride: usize, x: i32, y: i32, ch: u8, color: u32, font: &Font) {
    let g = font.glyph(ch);
    for (row, bits) in g.iter().enumerate() {
        for col in 0..8 {
            if (bits >> (7 - col)) & 1 == 1 {
                let px_x = x + col as i32;
                let px_y = y + row as i32;
                blend_put(px, stride, px_x, px_y, color, 220);
                blend_put(px, stride, px_x + 1, px_y, color, 72);
                blend_put(px, stride, px_x - 1, px_y, color, 72);
                blend_put(px, stride, px_x, px_y + 1, color, 72);
                blend_put(px, stride, px_x, px_y - 1, color, 72);
            }
        }
    }
}

fn put(px: &mut [u32], stride: usize, x: i32, y: i32, color: u32) {
    if x < 0 || y < 0 {
        return;
    }
    let x = x as usize;
    let y = y as usize;
    let h = px.len() / stride;
    if x >= stride || y >= h {
        return;
    }
    px[y * stride + x] = color;
}

fn parse_bdf(data: &[u8], glyphs: &mut [[u8; FONT_HEIGHT]; GLYPH_COUNT]) {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    let mut encoding: Option<usize> = None;
    let mut in_bitmap = false;
    let mut row = 0usize;

    for line in text.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("ENCODING ") {
            encoding = v.trim().parse::<usize>().ok();
            in_bitmap = false;
            row = 0;
        } else if line == "BITMAP" {
            in_bitmap = true;
            row = 0;
        } else if line == "ENDCHAR" {
            in_bitmap = false;
            encoding = None;
            row = 0;
        } else if in_bitmap
            && let Some(enc) = encoding
            && (ASCII_START..ASCII_END).contains(&enc)
            && row < FONT_HEIGHT
        {
            let idx = enc - ASCII_START;
            if let Ok(byte) = u8::from_str_radix(line, 16) {
                glyphs[idx][row] = byte;
            }
            row += 1;
        }
    }
}

fn binder_font() -> &'static Font {
    static FONT: OnceLock<Font> = OnceLock::new();
    FONT.get_or_init(Font::load)
}


fn find_kagami_tid() -> Option<u64> {
    for name in KAGAMI_PROCESS_CANDIDATES {
        if let Some(tid) = find_process_by_name(name) {
            return Some(tid);
        }
    }
    None
}

fn launch_dock(kagami_tid: u64) {
    let arg_tid = format!("--kagami-tid={}", kagami_tid);
    let args = [arg_tid.as_str()];
    match process::exec_with_args("/applications/Dock.app/entry.elf", &args) {
        Ok(pid) => println!("[Binder] launched Dock pid={}", pid),
        Err(_) => {
            eprintln!("[Binder] failed to launch Dock");
        }
    }
}

fn launch_app_window(
    kagami_tid: u64,
    exec_path: &str,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Option<DesktopWindow> {
    let arg_tid = format!("--kagami-tid={}", kagami_tid);
    let args = [arg_tid.as_str()];
    match process::exec_with_args(exec_path, &args) {
        Ok(pid) => {
            println!("[Binder] launched {} pid={}", app_title_from_exec_path(exec_path), pid);
            Some(DesktopWindow {
                x,
                y,
                width,
                height,
                title: app_title_from_exec_path(exec_path),
            })
        }
        Err(_) => {
            eprintln!("[Binder] failed to launch {}", app_title_from_exec_path(exec_path));
            None
        }
    }
}

fn app_title_from_exec_path(exec_path: &str) -> String {
    let parent = exec_path.trim_end_matches("/entry.elf");
    let stem = parent.rsplit('/').next().unwrap_or(parent);
    let stem = stem.strip_suffix(".app").unwrap_or(stem);
    if stem.is_empty() {
        "App".to_string()
    } else {
        stem.to_string()
    }
}

fn render_window_pixmap(title: &str, width: u32, height: u32) -> Vec<u32> {
    let component = VComponent::from_str(include_str!("components/window.html"))
        .width(width)
        .height(height)
        .text(title.to_string());
    render_component_to_pixmap(&component, width, height)
}

fn blit_pixmap(
    dst: &mut [u32],
    dst_stride: usize,
    dst_x: i32,
    dst_y: i32,
    src_width: usize,
    src_height: usize,
    src: &[u32],
) {
    for sy in 0..src_height {
        for sx in 0..src_width {
            let dx = dst_x + sx as i32;
            let dy = dst_y + sy as i32;
            if dx < 0 || dy < 0 {
                continue;
            }
            let dx = dx as usize;
            let dy = dy as usize;
            if dx >= dst_stride || dy * dst_stride + dx >= dst.len() {
                continue;
            }
            let src_idx = sy * src_width + sx;
            if src_idx >= src.len() {
                continue;
            }
            dst[dy * dst_stride + dx] = src[src_idx];
        }
    }
}

fn desktop_window_size() -> (u16, u16) {
    if let Some(info) = vga::get_info() {
        let w = info.width.clamp(1, u16::MAX as u32) as u16;
        let h = info.height.clamp(1, u16::MAX as u32) as u16;
        return (w, h);
    }
    (1280, 800)
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
