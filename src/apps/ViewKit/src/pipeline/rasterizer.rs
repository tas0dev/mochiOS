use super::display_list::{DisplayCommand, DisplayList};
use super::framebuffer::Framebuffer;
use super::image;

pub fn rasterize(display_list: &DisplayList, width: u32, height: u32) -> Framebuffer {
    let mut fb = Framebuffer::new(width, height);
    fb.clear(0x00000000);

    for item in &display_list.items {
        match item {
            DisplayCommand::FillRect {
                rect,
                color,
                radius,
                opacity,
            } => {
                fb.fill_rounded_rect(rect.x, rect.y, rect.width, rect.height, *radius, *color, *opacity);
            }
            DisplayCommand::DrawText {
                x,
                y,
                color,
                opacity,
                text,
            } => {
                rasterize_text(&mut fb, *x, *y, *color, *opacity, text);
            }
            DisplayCommand::DrawImage {
                rect,
                opacity,
                src,
                radius,
                fit_cover,
            } => {
                if let Some((pixels, w, h)) = image::load_image_from_path(src) {
                    if *fit_cover || *radius > 0 {
                        fb.blit_image_pixels_cover_rounded(
                            &pixels,
                            w,
                            h,
                            rect.x,
                            rect.y,
                            rect.width,
                            rect.height,
                            *radius,
                            *opacity,
                        );
                    } else {
                        fb.blit_image_pixels_fit(
                            &pixels,
                            w,
                            h,
                            rect.x,
                            rect.y,
                            rect.width,
                            rect.height,
                            *opacity,
                            0,
                        );
                    }
                } else {
                    // Debug fallback: show missing decode/load as magenta.
                    fb.fill_rect(rect.x, rect.y, rect.width, rect.height, 0xFFFF00FF, *opacity);
                }
            }
        }
    }

    fb
}

fn rasterize_text(fb: &mut Framebuffer, x: i32, y: i32, color: u32, opacity: f32, text: &str) {
    // Minimal text stub: draw one 6x10 block per character.
    let mut pen_x = x;
    for _ in text.chars() {
        fb.fill_rect(pen_x, y, 6, 10, color, opacity);
        pen_x += 8;
    }
}
