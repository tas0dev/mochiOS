//! 描画ラッパー（mochiOS / Linux host 共通）

use crate::vga::{self, FbInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GfxError {
    FramebufferUnavailable,
    InvalidSize,
}

pub struct Surface {
    fb_ptr: *mut u32,
    info: FbInfo,
}

impl Surface {
    /// mochiOS では実フレームバッファ、hosted-vga ではホストバッファへ接続する。
    pub fn from_system() -> Result<Self, GfxError> {
        let info = vga::get_info().ok_or(GfxError::FramebufferUnavailable)?;
        let fb_ptr = vga::map_framebuffer().ok_or(GfxError::FramebufferUnavailable)?;
        Ok(Self { fb_ptr, info })
    }

    /// Linux host 向けバッファを作成して Surface を返す。
    #[cfg(feature = "hosted-vga")]
    pub fn from_host(width: u32, height: u32) -> Result<Self, GfxError> {
        if width == 0 || height == 0 {
            return Err(GfxError::InvalidSize);
        }
        vga::host_init_framebuffer(width, height).map_err(|_| GfxError::FramebufferUnavailable)?;
        Self::from_system()
    }

    #[inline]
    pub fn width(&self) -> usize {
        self.info.width as usize
    }

    #[inline]
    pub fn height(&self) -> usize {
        self.info.height as usize
    }

    #[inline]
    pub fn stride(&self) -> usize {
        self.info.stride as usize
    }

    pub fn clear(&mut self, color: u32) {
        let c = with_alpha(color);
        let total = self.stride().saturating_mul(self.height());
        for i in 0..total {
            unsafe {
                self.fb_ptr.add(i).write_volatile(c);
            }
        }
    }

    pub fn fill_rect(&mut self, x: i32, y: i32, w: usize, h: usize, color: u32) {
        if w == 0 || h == 0 {
            return;
        }
        let (x0, x1) = clip_axis(x, w, self.width());
        let (y0, y1) = clip_axis(y, h, self.height());
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let c = with_alpha(color);
        let stride = self.stride();
        for yy in y0..y1 {
            let row = yy.saturating_mul(stride);
            for xx in x0..x1 {
                unsafe {
                    self.fb_ptr.add(row + xx).write_volatile(c);
                }
            }
        }
    }

    pub fn blit_argb(&mut self, x: i32, y: i32, width: usize, height: usize, pixels: &[u32]) {
        if width == 0 || height == 0 || pixels.len() < width.saturating_mul(height) {
            return;
        }
        let (x0, x1) = clip_axis(x, width, self.width());
        let (y0, y1) = clip_axis(y, height, self.height());
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let stride = self.stride();
        let src_x0 = x0 - x.max(0) as usize;
        let src_y0 = y0 - y.max(0) as usize;
        for yy in 0..(y1 - y0) {
            let dst_row = (y0 + yy).saturating_mul(stride);
            let src_row = (src_y0 + yy).saturating_mul(width);
            for xx in 0..(x1 - x0) {
                let src = with_alpha(pixels[src_row + src_x0 + xx]);
                unsafe {
                    self.fb_ptr.add(dst_row + x0 + xx).write_volatile(src);
                }
            }
        }
    }

    #[cfg(feature = "hosted-vga")]
    pub fn dump_ppm(&self, path: &str) -> Result<(), GfxError> {
        vga::host_dump_ppm(path).map_err(|_| GfxError::FramebufferUnavailable)
    }
}

#[inline]
fn with_alpha(color: u32) -> u32 {
    if (color >> 24) == 0 {
        color | 0xFF00_0000
    } else {
        color
    }
}

fn clip_axis(pos: i32, len: usize, max: usize) -> (usize, usize) {
    let start = pos.max(0) as usize;
    let end = (pos.saturating_add(len as i32)).max(0) as usize;
    (start.min(max), end.min(max))
}
