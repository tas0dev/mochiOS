#[derive(Debug, Clone)]
pub struct Framebuffer {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl Framebuffer {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0x00000000; (width * height) as usize],
        }
    }

    pub fn clear(&mut self, color: u32) {
        for p in &mut self.pixels {
            *p = color;
        }
    }

    pub fn blend_pixel(&mut self, x: i32, y: i32, color: u32, opacity: f32) {
        let Some(index) = self.pixel_index(x, y) else {
            return;
        };
        let dst = self.pixels[index];
        self.pixels[index] = blend_argb_over(dst, color, opacity);
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, width: i32, height: i32, color: u32, opacity: f32) {
        if width <= 0 || height <= 0 {
            return;
        }
        for yy in y..(y + height) {
            for xx in x..(x + width) {
                self.blend_pixel(xx, yy, color, opacity);
            }
        }
    }

    pub fn fill_rounded_rect(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        radius: i32,
        color: u32,
        opacity: f32,
    ) {
        if width <= 0 || height <= 0 {
            return;
        }
        let r = radius.max(0).min(width / 2).min(height / 2);
        if r == 0 {
            self.fill_rect(x, y, width, height, color, opacity);
            return;
        }

        let rf = r as f32;
        for yy in y..(y + height) {
            for xx in x..(x + width) {
                let lx = (xx - x) as f32;
                let ly = (yy - y) as f32;
                let coverage = rounded_rect_coverage(lx, ly, width as f32, height as f32, rf);
                if coverage > 0.0 {
                    self.blend_pixel(xx, yy, color, opacity * coverage);
                }
            }
        }
    }

    pub fn blit_image_pixels(
        &mut self,
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
                
                if dst_x < 0 || dst_y < 0 || dst_x >= self.width as i32 || dst_y >= self.height as i32 {
                    continue;
                }
                
                let src_idx = (src_y * src_width as i32 + src_x) as usize;
                let dst_idx = (dst_y * self.width as i32 + dst_x) as usize;
                
                if src_idx >= src_pixels.len() || dst_idx >= self.pixels.len() {
                    continue;
                }
                
                let src = src_pixels[src_idx];
                self.pixels[dst_idx] = blend_argb_over(self.pixels[dst_idx], src, opacity);
            }
        }
    }

    pub fn blit_image_pixels_fit(
        &mut self,
        src_pixels: &[u32],
        src_width: u32,
        src_height: u32,
        dst_x: i32,
        dst_y: i32,
        dst_width: i32,
        dst_height: i32,
        opacity: f32,
        padding: i32,
    ) {
        if src_width == 0 || src_height == 0 || dst_width <= 0 || dst_height <= 0 {
            return;
        }

        let avail_w = (dst_width - padding * 2).max(1) as f32;
        let avail_h = (dst_height - padding * 2).max(1) as f32;
        let scale = (avail_w / src_width as f32)
            .min(avail_h / src_height as f32)
            .min(1.0);
        if scale <= 0.0 {
            return;
        }

        let out_w = (src_width as f32 * scale).round().max(1.0) as i32;
        let out_h = (src_height as f32 * scale).round().max(1.0) as i32;
        let start_x = dst_x + (dst_width - out_w) / 2;
        let start_y = dst_y + (dst_height - out_h) / 2;

        for oy in 0..out_h {
            let sy = ((oy as f32) / scale).floor() as u32;
            let sy = sy.min(src_height - 1);
            for ox in 0..out_w {
                let sx = ((ox as f32) / scale).floor() as u32;
                let sx = sx.min(src_width - 1);
                let dx = start_x + ox;
                let dy = start_y + oy;
                if dx < 0 || dy < 0 || dx >= self.width as i32 || dy >= self.height as i32 {
                    continue;
                }
                let src_idx = (sy * src_width + sx) as usize;
                let dst_idx = (dy as u32 * self.width + dx as u32) as usize;
                if src_idx >= src_pixels.len() || dst_idx >= self.pixels.len() {
                    continue;
                }
                self.pixels[dst_idx] = blend_argb_over(self.pixels[dst_idx], src_pixels[src_idx], opacity);
            }
        }
    }

    pub fn blit_image_pixels_cover_rounded(
        &mut self,
        src_pixels: &[u32],
        src_width: u32,
        src_height: u32,
        dst_x: i32,
        dst_y: i32,
        dst_width: i32,
        dst_height: i32,
        radius: i32,
        opacity: f32,
    ) {
        if src_width == 0 || src_height == 0 || dst_width <= 0 || dst_height <= 0 {
            return;
        }

        let scale = (dst_width as f32 / src_width as f32).max(dst_height as f32 / src_height as f32);
        let out_w = (src_width as f32 * scale).ceil().max(1.0) as i32;
        let out_h = (src_height as f32 * scale).ceil().max(1.0) as i32;
        let start_x = dst_x + (dst_width - out_w) / 2;
        let start_y = dst_y + (dst_height - out_h) / 2;
        let clip_radius = radius.max(0).min(dst_width / 2).min(dst_height / 2);
        let clip_radius_f = clip_radius as f32;

        for dy in 0..dst_height {
            let py = dst_y + dy;
            for dx in 0..dst_width {
                let px = dst_x + dx;
                let coverage = rounded_rect_coverage(dx as f32, dy as f32, dst_width as f32, dst_height as f32, clip_radius_f);
                if coverage <= 0.0 {
                    continue;
                }

                let src_x = (((px - start_x) as f32 + 0.5) / scale).floor() as i32;
                let src_y = (((py - start_y) as f32 + 0.5) / scale).floor() as i32;
                if src_x < 0 || src_y < 0 || src_x >= src_width as i32 || src_y >= src_height as i32 {
                    continue;
                }

                let src_idx = (src_y as u32 * src_width + src_x as u32) as usize;
                let dst_idx = match self.pixel_index(px, py) {
                    Some(idx) => idx,
                    None => continue,
                };
                if src_idx >= src_pixels.len() {
                    continue;
                }
                self.pixels[dst_idx] = blend_argb_over(self.pixels[dst_idx], src_pixels[src_idx], opacity * coverage);
            }
        }
    }

    fn pixel_index(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 {
            return None;
        }
        let x = x as u32;
        let y = y as u32;
        if x >= self.width || y >= self.height {
            return None;
        }
        Some((y * self.width + x) as usize)
    }
}

fn rounded_rect_coverage(px: f32, py: f32, width: f32, height: f32, radius: f32) -> f32 {
    // 4x MSAA pattern
    const OFFSETS: [(f32, f32); 4] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];
    let mut inside = 0_u32;
    for (ox, oy) in OFFSETS {
        if is_inside_rounded_rect_at(px + ox, py + oy, width, height, radius) {
            inside += 1;
        }
    }
    inside as f32 / OFFSETS.len() as f32
}

fn is_inside_rounded_rect_at(x: f32, y: f32, w: f32, h: f32, radius: f32) -> bool {

    if x >= radius && x <= (w - radius) {
        return true;
    }
    if y >= radius && y <= (h - radius) {
        return true;
    }

    let tl = (x - radius, y - radius);
    let tr = (x - (w - radius), y - radius);
    let bl = (x - radius, y - (h - radius));
    let br = (x - (w - radius), y - (h - radius));

    let rr = radius * radius;
    (tl.0 * tl.0 + tl.1 * tl.1 <= rr)
        || (tr.0 * tr.0 + tr.1 * tr.1 <= rr)
        || (bl.0 * bl.0 + bl.1 * bl.1 <= rr)
        || (br.0 * br.0 + br.1 * br.1 <= rr)
}

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
