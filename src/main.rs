use fontdue::{
    layout::{CoordinateSystem, Layout, LayoutSettings, TextStyle},
    Font, FontSettings,
};
use swiftlib::{
    ipc::{ipc_recv, ipc_send},
    keyboard::{read_scancode, read_scancode_tap},
    privileged,
    task::{find_process_by_name, yield_now},
};

const IPC_BUF_SIZE: usize = 4128;
const KAGAMI_PROCESS_CANDIDATES: [&str; 3] =
    ["/Applications/Kagami.app/entry.elf", "Kagami.app", "entry.elf"];

const OP_REQ_CREATE_WINDOW: u32 = 1;
const OP_RES_WINDOW_CREATED: u32 = 2;
const OP_REQ_ATTACH_SHARED: u32 = 5;
const OP_REQ_PRESENT_SHARED: u32 = 6;
const OP_RES_SHARED_ATTACHED: u32 = 7;
const LAYER_APP: u8 = 1;
const UI_FONT_PATH: &str = "/Resources/fonts/NotoSansJP-Regular.ttf";

struct UiFont {
    font: Font,
    px: f32,
}

struct SharedSurface {
    virt_addr: u64,
    page_count: u64,
    total_pixels: usize,
}

impl UiFont {
    fn load(path: &str, px: f32) -> Result<Self, &'static str> {
        let data = std::fs::read(path).map_err(|_| "font read failed")?;
        let font = Font::from_bytes(data, FontSettings::default()).map_err(|_| "font parse failed")?;
        Ok(Self { font, px })
    }
}

fn main() {
    println!("[Terminal] start");
    let kagami_tid = match parse_kagami_tid_from_args().or_else(find_kagami_tid) {
        Some(tid) => tid,
        None => {
            eprintln!("[Terminal] Kagami not found");
            return;
        }
    };

    let width: u16 = 720;
    let height: u16 = 420;
    let window_id = match create_window(kagami_tid, width, height) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("[Terminal] create window failed: {}", e);
            return;
        }
    };

    let ui_font = match UiFont::load(UI_FONT_PATH, 18.0) {
        Ok(font) => Some(font),
        Err(e) => {
            eprintln!("[Terminal] failed to load {}: {} (fallback draw)", UI_FONT_PATH, e);
            None
        }
    };

    let pixels = render_terminal_bootstrap(width as usize, height as usize, ui_font.as_ref());
    if let Err(e) = flush_window_shared(kagami_tid, window_id, width, height, &pixels) {
        eprintln!("[Terminal] draw failed: {}", e);
        return;
    }
    println!("[Terminal] window shown (shared)");

    loop {
        let sc_opt = match read_scancode_tap() {
            Ok(Some(sc)) => Some(sc),
            Ok(None) => read_scancode(),
            Err(_) => read_scancode(),
        };
        if let Some(sc) = sc_opt
            && (sc == 0x01 || sc == 0x81)
        {
            println!("[Terminal] exit");
            return;
        }
        yield_now();
    }
}

fn create_window(kagami_tid: u64, width: u16, height: u16) -> Result<u32, &'static str> {
    let mut req = [0u8; 9];
    req[0..4].copy_from_slice(&OP_REQ_CREATE_WINDOW.to_le_bytes());
    req[4..6].copy_from_slice(&width.to_le_bytes());
    req[6..8].copy_from_slice(&height.to_le_bytes());
    req[8] = LAYER_APP;
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

fn flush_window_shared(
    kagami_tid: u64,
    window_id: u32,
    width: u16,
    height: u16,
    pixels: &[u32],
) -> Result<SharedSurface, &'static str> {
    let total = width as usize * height as usize;
    if pixels.len() < total {
        return Err("pixel buffer too small");
    }
    let total_bytes = total.checked_mul(4).ok_or("size overflow")?;
    let page_count = total_bytes.div_ceil(4096);
    if page_count == 0 {
        return Err("shared surface page count out of range");
    }

    let mut phys_pages = vec![0u64; page_count];
    let virt_addr =
        unsafe { privileged::alloc_shared_pages(page_count as u64, Some(phys_pages.as_mut_slice()), 0) };
    if (virt_addr as i64) < 0 || virt_addr == 0 {
        return Err("alloc_shared_pages failed");
    }
    let surface = SharedSurface {
        virt_addr,
        page_count: page_count as u64,
        total_pixels: total,
    };
    blit_shared_surface(&surface, pixels);

    let mut attach = [0u8; 12];
    attach[0..4].copy_from_slice(&OP_REQ_ATTACH_SHARED.to_le_bytes());
    attach[4..8].copy_from_slice(&window_id.to_le_bytes());
    attach[8..10].copy_from_slice(&width.to_le_bytes());
    attach[10..12].copy_from_slice(&height.to_le_bytes());
    if (ipc_send(kagami_tid, &attach) as i64) < 0 {
        return Err("failed to send shared attach");
    }

    let send_pages_ret = unsafe { privileged::ipc_send_pages(kagami_tid, phys_pages.as_slice(), 0) };
    if (send_pages_ret as i64) < 0 {
        return Err("failed to send shared pages");
    }
    wait_shared_attach_ack(kagami_tid, window_id)?;
    for _ in 0..3 {
        present_shared(kagami_tid, window_id)?;
        yield_now();
    }
    Ok(surface)
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

fn present_shared(kagami_tid: u64, window_id: u32) -> Result<(), &'static str> {
    let mut present = [0u8; 8];
    present[0..4].copy_from_slice(&OP_REQ_PRESENT_SHARED.to_le_bytes());
    present[4..8].copy_from_slice(&window_id.to_le_bytes());
    if (ipc_send(kagami_tid, &present) as i64) < 0 {
        return Err("failed to send shared present");
    }
    Ok(())
}

fn render_terminal_bootstrap(width: usize, height: usize, ui_font: Option<&UiFont>) -> Vec<u32> {
    let mut px = vec![0u32; width * height];
    for y in 0..height {
        let row = y * width;
        for x in 0..width {
            let shade = (((x + y) % 24) as u32) * 2;
            let c = 0xFF00_0000 | ((18 + shade) << 16) | ((20 + shade) << 8) | (24 + shade);
            px[row + x] = c;
        }
    }

    fill_rect(&mut px, width, 0, 0, width as i32, 34, 0xFF1D_2330);
    fill_rect(
        &mut px,
        width,
        10,
        8,
        width as i32 - 20,
        height as i32 - 18,
        0xFF0D_1117,
    );
    if let Some(font) = ui_font {
        draw_text(&mut px, width, font, 16, 6, "Terminal", 0xFFCF_D8E3);
        draw_text(
            &mut px,
            width,
            font,
            24,
            50,
            "NotoSansJP-Regular.ttf loaded.",
            0xFFA6_B3C2,
        );
        draw_text(
            &mut px,
            width,
            font,
            24,
            24 + 26,
            "フォント: /Resources/fonts/NotoSansJP-Regular.ttf",
            0xFF9E_B8E8,
        );
        draw_text(
            &mut px,
            width,
            font,
            24,
            24 + 52,
            "Press Esc to close this test window.",
            0xFF7D_8CA1,
        );
    } else {
        fill_rect(&mut px, width, 16, 10, 160, 16, 0xFF4A_5B70);
        fill_rect(&mut px, width, 24, 52, 360, 10, 0xFF82_8F9C);
        fill_rect(&mut px, width, 24, 78, 480, 10, 0xFF66_7280);
    }
    px
}

fn fill_rect(px: &mut [u32], stride: usize, x: i32, y: i32, w: i32, h: i32, color: u32) {
    if w <= 0 || h <= 0 {
        return;
    }
    let height = px.len() / stride;
    let x0 = x.max(0) as usize;
    let y0 = y.max(0) as usize;
    let x1 = (x + w).max(0) as usize;
    let y1 = (y + h).max(0) as usize;
    let x1 = x1.min(stride);
    let y1 = y1.min(height);
    for yy in y0..y1 {
        let row = yy * stride;
        for xx in x0..x1 {
            px[row + xx] = color;
        }
    }
}

fn draw_text(
    px: &mut [u32],
    stride: usize,
    ui_font: &UiFont,
    x: i32,
    y: i32,
    text: &str,
    color: u32,
) {
    let mut layout = Layout::new(CoordinateSystem::PositiveYDown);
    layout.reset(&LayoutSettings {
        x: x as f32,
        y: y as f32,
        ..LayoutSettings::default()
    });
    layout.append(&[&ui_font.font], &TextStyle::new(text, ui_font.px, 0));

    let height = px.len() / stride;
    for glyph in layout.glyphs() {
        let (metrics, bitmap) = ui_font.font.rasterize_config(glyph.key);
        if metrics.width == 0 || metrics.height == 0 {
            continue;
        }
        let gx = glyph.x as i32;
        let gy = glyph.y as i32;
        for row in 0..metrics.height {
            let yy = gy + row as i32;
            if yy < 0 || yy as usize >= height {
                continue;
            }
            for col in 0..metrics.width {
                let xx = gx + col as i32;
                if xx < 0 || xx as usize >= stride {
                    continue;
                }
                let cov = bitmap[row * metrics.width + col];
                if cov == 0 {
                    continue;
                }
                let idx = yy as usize * stride + xx as usize;
                px[idx] = blend_rgb(px[idx], color, cov);
            }
        }
    }
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

fn find_kagami_tid() -> Option<u64> {
    for name in KAGAMI_PROCESS_CANDIDATES {
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
