use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[path = "../../Kagami/src/libkagami.rs"]
mod libkagami;

pub fn main() -> Result<(), String> {
    const WIDTH: i32 = 1280;
    const HEIGHT: i32 = 800;

    let mut host = libkagami::host_HostDisplay::new()?;
    let mut surface = host.create_surface(WIDTH, HEIGHT)?;
    host.set_toplevel(&mut surface)?;

    draw_desktop(surface.back_buffer_mut(), WIDTH as usize, HEIGHT as usize);
    surface.swap_and_commit()?;

    let frame_done = Arc::new(AtomicBool::new(false));
    loop {
        frame_done.store(false, Ordering::SeqCst);
        surface.request_frame(frame_done.clone())?;
        surface.commit_front()?;
        while !frame_done.load(Ordering::SeqCst) {
            host.dispatch()?;
            std::thread::sleep(Duration::from_millis(8));
        }
    }
}

fn draw_desktop(buf: &mut [u8], width: usize, height: usize) {
    // Wallpaper gradient
    for y in 0..height {
        let t = y as f32 / (height.max(1) as f32);
        let r = lerp(222.0, 168.0, t) as u8;
        let g = lerp(232.0, 179.0, t) as u8;
        let b = lerp(244.0, 201.0, t) as u8;
        for x in 0..width {
            put(buf, width, x as i32, y as i32, r, g, b, 0xFF);
        }
    }

    // Status bar
    fill_rect(buf, width, 0, 0, width as i32, 34, (0xF4, 0xF7, 0xFA, 0xFF));

    // Demo app windows (Binder as DE/WM preview)
    fill_rounded_rect(buf, width, 130, 95, 520, 340, 10, (0xF8, 0xFA, 0xFD, 0xFF));
    fill_rect(buf, width, 130, 95, 520, 28, (0xE7, 0xEA, 0xF2, 0xFF));
    fill_rounded_rect(buf, width, 700, 170, 440, 280, 10, (0xF7, 0xF8, 0xFB, 0xFF));
    fill_rect(buf, width, 700, 170, 440, 28, (0xE7, 0xEA, 0xF2, 0xFF));
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

fn put(buf: &mut [u8], stride_pixels: usize, x: i32, y: i32, r: u8, g: u8, b: u8, a: u8) {
    if x < 0 || y < 0 {
        return;
    }
    let x = x as usize;
    let y = y as usize;
    if x >= stride_pixels {
        return;
    }
    let h = buf.len() / (stride_pixels * 4);
    if y >= h {
        return;
    }
    let i = (y * stride_pixels + x) * 4;
    buf[i] = b;
    buf[i + 1] = g;
    buf[i + 2] = r;
    buf[i + 3] = a;
}

fn fill_rect(
    buf: &mut [u8],
    stride_pixels: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    color: (u8, u8, u8, u8),
) {
    if w <= 0 || h <= 0 {
        return;
    }
    for yy in y..(y + h) {
        for xx in x..(x + w) {
            put(buf, stride_pixels, xx, yy, color.0, color.1, color.2, color.3);
        }
    }
}

fn fill_rounded_rect(
    buf: &mut [u8],
    stride_pixels: usize,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    radius: i32,
    color: (u8, u8, u8, u8),
) {
    let r = radius.max(0).min(w / 2).min(h / 2);
    for yy in 0..h {
        for xx in 0..w {
            if inside_rounded_rect(xx as f32 + 0.5, yy as f32 + 0.5, w as f32, h as f32, r as f32)
            {
                put(
                    buf,
                    stride_pixels,
                    x + xx,
                    y + yy,
                    color.0,
                    color.1,
                    color.2,
                    color.3,
                );
            }
        }
    }
}

fn inside_rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> bool {
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
