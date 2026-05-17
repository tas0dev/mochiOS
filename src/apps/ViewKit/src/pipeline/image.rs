use image::{DynamicImage, ImageReader};
use std::io::Cursor;
#[cfg(all(target_os = "linux", target_env = "musl"))]
use swiftlib::fs;

/// 画像ファイルを読み込み ARGB32 ピクセル配列に変換
pub fn load_image_from_bytes(data: &[u8]) -> Option<(Vec<u32>, u32, u32)> {
    let cursor = Cursor::new(data);
    let reader = ImageReader::new(cursor);
    let reader = reader.with_guessed_format().ok()?;
    let img = reader.decode().ok()?;
    image_to_pixels(&img)
}

/// ファイルパスから画像を読み込み ARGB32 ピクセル配列に変換
pub fn load_image_from_path(path: &str) -> Option<(Vec<u32>, u32, u32)> {
    #[cfg(all(target_os = "linux", target_env = "musl"))]
    {
        let data = fs::read_file_via_fs(path, 512 * 1024).ok()??;
        return load_image_from_bytes(&data);
    }

    #[cfg(not(all(target_os = "linux", target_env = "musl")))]
    {
        let data = std::fs::read(path).ok()?;
        return load_image_from_bytes(&data);
    }
}

/// DynamicImage を ARGB32 ピクセル配列に変換
pub fn image_to_pixels(img: &DynamicImage) -> Option<(Vec<u32>, u32, u32)> {
    let rgba = img.to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    
    let pixels = rgba
        .chunks_exact(4)
        .map(|chunk| {
            let r = chunk[0] as u32;
            let g = chunk[1] as u32;
            let b = chunk[2] as u32;
            let a = chunk[3] as u32;
            (a << 24) | (r << 16) | (g << 8) | b
        })
        .collect();
    
    Some((pixels, width, height))
}

/// 画像をフレームバッファにブレンディング
pub fn blit_image(
    dst_pixels: &mut [u32],
    dst_width: u32,
    dst_height: u32,
    src_pixels: &[u32],
    src_width: u32,
    src_height: u32,
    x: i32,
    y: i32,
    opacity: f32,
) {
    let opacity = opacity.clamp(0.0, 1.0);
    
    for src_y in 0..src_height as i32 {
        for src_x in 0..src_width as i32 {
            let dst_x = x + src_x;
            let dst_y = y + src_y;
            
            if dst_x < 0 || dst_y < 0 || dst_x >= dst_width as i32 || dst_y >= dst_height as i32 {
                continue;
            }
            
            let src_idx = (src_y * src_width as i32 + src_x) as usize;
            let dst_idx = (dst_y * dst_width as i32 + dst_x) as usize;
            
            if src_idx >= src_pixels.len() || dst_idx >= dst_pixels.len() {
                continue;
            }
            
            let src = src_pixels[src_idx];
            let dst = dst_pixels[dst_idx];
            
            dst_pixels[dst_idx] = blend_argb_over(dst, src, opacity);
        }
    }
}

/// ARGB ブレンディング (over)
fn blend_argb_over(dst: u32, src: u32, opacity: f32) -> u32 {
    let opacity = opacity.clamp(0.0, 1.0);
    let src_a = ((src >> 24) & 0xff) as f32 / 255.0;
    let a = (src_a * opacity).clamp(0.0, 1.0);
    
    if a <= 0.0 {
        return dst;
    }
    
    let da = ((dst >> 24) & 0xff) as f32 / 255.0;
    let dr = ((dst >> 16) & 0xff) as f32;
    let dg = ((dst >> 8) & 0xff) as f32;
    let db = (dst & 0xff) as f32;
    
    let sr = ((src >> 16) & 0xff) as f32;
    let sg = ((src >> 8) & 0xff) as f32;
    let sb = (src & 0xff) as f32;
    
    let out_r = (sr * a + dr * (1.0 - a)).round().clamp(0.0, 255.0) as u32;
    let out_g = (sg * a + dg * (1.0 - a)).round().clamp(0.0, 255.0) as u32;
    let out_b = (sb * a + db * (1.0 - a)).round().clamp(0.0, 255.0) as u32;
    let out_a = (a + da * (1.0 - a)).round().clamp(0.0, 1.0) * 255.0;

    (((out_a.round() as u32) & 0xff) << 24) | (out_r << 16) | (out_g << 8) | out_b
}
