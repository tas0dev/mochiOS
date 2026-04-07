use alloc::vec;
use alloc::vec::Vec;
use core::cmp::min;

const BOOTING_GIF: &[u8] = include_bytes!("../resources/Resources/booting.gif");
const MARGIN_PX: usize = 16;
const TICK_QUANTUM_MS: u16 = 6;

#[derive(Clone)]
struct GifFrame {
    delay_ms: u16,
    pixels: Vec<u32>,
}

#[derive(Clone, Copy)]
struct GraphicControl {
    delay_cs: u16,
    transparent_index: Option<u8>,
}

impl Default for GraphicControl {
    fn default() -> Self {
        Self {
            delay_cs: 7, // 70ms
            transparent_index: None,
        }
    }
}

pub struct BootingGifPlayer {
    fb: *mut u32,
    fb_len: usize,
    screen_w: usize,
    screen_h: usize,
    stride: usize,
    frame_w: usize,
    frame_h: usize,
    pos_x: usize,
    pos_y: usize,
    frames: Vec<GifFrame>,
    next_frame: usize,
    elapsed_ms: u16,
    started: bool,
}

impl BootingGifPlayer {
    pub fn new(
        fb: *mut u32,
        screen_w: usize,
        screen_h: usize,
        stride: usize,
    ) -> Result<Self, &'static str> {
        let (frame_w, frame_h, frames) = decode_gif(BOOTING_GIF)?;
        if frames.is_empty() {
            return Err("no frames");
        }
        let fb_len = screen_h
            .checked_mul(stride)
            .ok_or("framebuffer size overflow")?;
        let pos_x = screen_w.saturating_sub(frame_w + MARGIN_PX);
        let pos_y = screen_h.saturating_sub(frame_h + MARGIN_PX);
        Ok(Self {
            fb,
            fb_len,
            screen_w,
            screen_h,
            stride,
            frame_w,
            frame_h,
            pos_x,
            pos_y,
            frames,
            next_frame: 0,
            elapsed_ms: 0,
            started: false,
        })
    }

    pub fn tick(&mut self) {
        if self.fb.is_null() || self.frames.is_empty() {
            return;
        }
        if !self.started {
            self.started = true;
            let frame = &self.frames[self.next_frame];
            self.draw_frame(frame);
            return;
        }

        self.elapsed_ms = self.elapsed_ms.saturating_add(TICK_QUANTUM_MS);
        let mut advanced = false;
        loop {
            let delay_ms = self.frames[self.next_frame].delay_ms.max(TICK_QUANTUM_MS);
            if self.elapsed_ms < delay_ms {
                break;
            }
            self.elapsed_ms -= delay_ms;
            self.next_frame = (self.next_frame + 1) % self.frames.len();
            advanced = true;
        }

        if advanced {
            let frame = &self.frames[self.next_frame];
            self.draw_frame(frame);
        }
    }

    fn draw_frame(&self, frame: &GifFrame) {
        let draw_w = min(self.frame_w, self.screen_w.saturating_sub(self.pos_x));
        let draw_h = min(self.frame_h, self.screen_h.saturating_sub(self.pos_y));
        for y in 0..draw_h {
            let src_row_start = match y.checked_mul(self.frame_w) {
                Some(v) => v,
                None => return,
            };
            if src_row_start >= frame.pixels.len() {
                continue;
            }
            let src_available = frame.pixels.len() - src_row_start;
            let row_draw_w = min(draw_w, src_available);
            if row_draw_w == 0 {
                continue;
            }
            let dst_row_start = match self
                .pos_y
                .checked_add(y)
                .and_then(|row| row.checked_mul(self.stride))
                .and_then(|base| base.checked_add(self.pos_x))
            {
                Some(v) => v,
                None => return,
            };
            let dst_row_end = match dst_row_start.checked_add(row_draw_w) {
                Some(v) => v,
                None => return,
            };
            if dst_row_end > self.fb_len {
                return;
            }
            unsafe {
                core::ptr::copy_nonoverlapping(
                    frame.pixels.as_ptr().add(src_row_start),
                    self.fb.add(dst_row_start),
                    row_draw_w,
                );
            }
        }
    }
}

fn decode_gif(data: &[u8]) -> Result<(usize, usize, Vec<GifFrame>), &'static str> {
    if data.len() < 13 {
        return Err("gif too small");
    }
    if &data[0..3] != b"GIF" {
        return Err("not a GIF file");
    }

    let screen_w = read_u16(data, 6)? as usize;
    let screen_h = read_u16(data, 8)? as usize;
    if screen_w == 0 || screen_h == 0 {
        return Err("invalid GIF size");
    }

    let packed = data[10];
    let has_gct = (packed & 0x80) != 0;
    let gct_entries = if has_gct {
        1usize << ((packed & 0x07) + 1)
    } else {
        0
    };
    let bg_index = data[11] as usize;

    let mut pos = 13usize;
    let mut global_palette = [0u32; 256];
    if has_gct {
        let need = gct_entries.checked_mul(3).ok_or("gif palette overflow")?;
        if pos + need > data.len() || gct_entries > 256 {
            return Err("invalid global palette");
        }
        for i in 0..gct_entries {
            let r = data[pos + i * 3];
            let g = data[pos + i * 3 + 1];
            let b = data[pos + i * 3 + 2];
            global_palette[i] = rgb_to_u32(r, g, b);
        }
        pos += need;
    }
    let background_color = global_palette.get(bg_index).copied().unwrap_or(0);
    let pixel_count = screen_w
        .checked_mul(screen_h)
        .ok_or("gif canvas too large")?;
    let mut canvas = vec![background_color; pixel_count];

    let mut frames = Vec::new();
    let mut control = GraphicControl::default();

    while pos < data.len() {
        let block = data[pos];
        pos += 1;
        match block {
            0x3B => break, // trailer
            0x21 => {
                let label = *data.get(pos).ok_or("broken extension block")?;
                pos += 1;
                if label == 0xF9 {
                    let block_size = *data.get(pos).ok_or("broken GCE")? as usize;
                    pos += 1;
                    if block_size != 4 || pos + 4 > data.len() {
                        return Err("invalid GCE");
                    }
                    let gce_packed = data[pos];
                    let delay_cs = u16::from_le_bytes([data[pos + 1], data[pos + 2]]);
                    let transparent_index = if (gce_packed & 0x01) != 0 {
                        Some(data[pos + 3])
                    } else {
                        None
                    };
                    pos += 4;
                    if *data.get(pos).ok_or("broken GCE terminator")? != 0 {
                        return Err("invalid GCE terminator");
                    }
                    pos += 1;
                    control = GraphicControl {
                        delay_cs: if delay_cs == 0 { 7 } else { delay_cs },
                        transparent_index,
                    };
                } else {
                    skip_sub_blocks(data, &mut pos)?;
                }
            }
            0x2C => {
                if pos + 9 > data.len() {
                    return Err("broken image descriptor");
                }
                let left = read_u16(data, pos)? as usize;
                let top = read_u16(data, pos + 2)? as usize;
                let width = read_u16(data, pos + 4)? as usize;
                let height = read_u16(data, pos + 6)? as usize;
                let img_packed = data[pos + 8];
                pos += 9;

                if width == 0 || height == 0 {
                    return Err("invalid image descriptor size");
                }

                let has_lct = (img_packed & 0x80) != 0;
                let interlaced = (img_packed & 0x40) != 0;
                let mut active_palette = global_palette;
                if has_lct {
                    let lct_entries = 1usize << ((img_packed & 0x07) + 1);
                    let need = lct_entries
                        .checked_mul(3)
                        .ok_or("gif local palette overflow")?;
                    if pos + need > data.len() || lct_entries > 256 {
                        return Err("invalid local palette");
                    }
                    for i in 0..lct_entries {
                        let r = data[pos + i * 3];
                        let g = data[pos + i * 3 + 1];
                        let b = data[pos + i * 3 + 2];
                        active_palette[i] = rgb_to_u32(r, g, b);
                    }
                    pos += need;
                }

                let lzw_min_code_size = *data.get(pos).ok_or("missing LZW min code size")?;
                pos += 1;
                let compressed = collect_sub_blocks(data, &mut pos)?;
                let expected = width
                    .checked_mul(height)
                    .ok_or("frame size overflow while decoding GIF")?;
                let mut indices = decode_lzw(lzw_min_code_size, &compressed, expected)?;
                if interlaced {
                    indices = deinterlace(indices, width, height)?;
                }

                for y in 0..height {
                    let dst_y = top + y;
                    if dst_y >= screen_h {
                        continue;
                    }
                    let src_row = y * width;
                    let dst_row = dst_y * screen_w;
                    for x in 0..width {
                        let dst_x = left + x;
                        if dst_x >= screen_w {
                            continue;
                        }
                        let idx = indices[src_row + x];
                        if control.transparent_index == Some(idx) {
                            continue;
                        }
                        canvas[dst_row + dst_x] = active_palette[idx as usize];
                    }
                }

                let delay_ms = control.delay_cs.saturating_mul(10).max(10);
                frames.push(GifFrame {
                    delay_ms,
                    pixels: canvas.clone(),
                });
                control = GraphicControl::default();
            }
            _ => return Err("unknown GIF block"),
        }
    }

    if frames.is_empty() {
        return Err("GIF has no image frames");
    }
    Ok((screen_w, screen_h, frames))
}

fn rgb_to_u32(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

fn read_u16(data: &[u8], offset: usize) -> Result<u16, &'static str> {
    let b0 = *data.get(offset).ok_or("u16 read overflow")?;
    let b1 = *data.get(offset + 1).ok_or("u16 read overflow")?;
    Ok(u16::from_le_bytes([b0, b1]))
}

fn skip_sub_blocks(data: &[u8], pos: &mut usize) -> Result<(), &'static str> {
    loop {
        let len = *data.get(*pos).ok_or("broken sub-block length")? as usize;
        *pos += 1;
        if len == 0 {
            return Ok(());
        }
        if *pos + len > data.len() {
            return Err("sub-block overflow");
        }
        *pos += len;
    }
}

fn collect_sub_blocks(data: &[u8], pos: &mut usize) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::new();
    loop {
        let len = *data.get(*pos).ok_or("broken image sub-block length")? as usize;
        *pos += 1;
        if len == 0 {
            break;
        }
        if *pos + len > data.len() {
            return Err("image sub-block overflow");
        }
        out.extend_from_slice(&data[*pos..*pos + len]);
        *pos += len;
    }
    Ok(out)
}

fn read_code(data: &[u8], bit_pos: &mut usize, code_size: usize) -> Option<u16> {
    let mut value = 0u16;
    for i in 0..code_size {
        let idx = *bit_pos + i;
        let byte = *data.get(idx / 8)?;
        let bit = (byte >> (idx % 8)) & 1;
        value |= (bit as u16) << i;
    }
    *bit_pos += code_size;
    Some(value)
}

fn expand_code(
    mut code: usize,
    clear_code: usize,
    prefix: &[u16; 4096],
    suffix: &[u8; 4096],
    stack: &mut [u8; 4096],
) -> Result<(usize, u8), &'static str> {
    let mut len = 0usize;
    while code >= clear_code {
        if code >= 4096 || len >= stack.len() {
            return Err("LZW dictionary overflow");
        }
        stack[len] = suffix[code];
        len += 1;
        code = prefix[code] as usize;
    }
    let first = code as u8;
    if len >= stack.len() {
        return Err("LZW stack overflow");
    }
    stack[len] = first;
    len += 1;
    Ok((len, first))
}

fn decode_lzw(
    min_code_size: u8,
    data: &[u8],
    expected_len: usize,
) -> Result<Vec<u8>, &'static str> {
    if min_code_size == 0 || min_code_size > 8 {
        return Err("unsupported LZW min code size");
    }
    let clear_code = 1usize << min_code_size;
    let end_code = clear_code + 1;
    let mut code_size = min_code_size as usize + 1;
    let mut next_code = end_code + 1;

    let mut prefix = [0u16; 4096];
    let mut suffix = [0u8; 4096];
    for i in 0..clear_code {
        suffix[i] = i as u8;
    }

    let mut bit_pos = 0usize;
    let mut output = Vec::with_capacity(expected_len);
    let mut stack = [0u8; 4096];
    let mut prev_code: Option<usize> = None;

    while let Some(code_raw) = read_code(data, &mut bit_pos, code_size) {
        let code = code_raw as usize;
        if code == clear_code {
            code_size = min_code_size as usize + 1;
            next_code = end_code + 1;
            prev_code = None;
            continue;
        }
        if code == end_code {
            break;
        }

        let (mut stack_len, first) = if code < next_code {
            expand_code(code, clear_code, &prefix, &suffix, &mut stack)?
        } else if code == next_code {
            let prev = prev_code.ok_or("broken LZW stream")?;
            let (mut prev_len, first) =
                expand_code(prev, clear_code, &prefix, &suffix, &mut stack)?;
            if prev_len >= stack.len() {
                return Err("LZW stack overflow");
            }
            stack[prev_len] = first;
            prev_len += 1;
            (prev_len, first)
        } else {
            return Err("invalid LZW code");
        };

        while stack_len > 0 {
            stack_len -= 1;
            output.push(stack[stack_len]);
            if output.len() >= expected_len {
                break;
            }
        }

        if let Some(prev) = prev_code {
            if next_code < 4096 {
                prefix[next_code] = prev as u16;
                suffix[next_code] = first;
                next_code += 1;
                if next_code == (1usize << code_size) && code_size < 12 {
                    code_size += 1;
                }
            }
        }
        prev_code = Some(code);

        if output.len() >= expected_len {
            break;
        }
    }

    if output.len() < expected_len {
        return Err("LZW output truncated");
    }
    output.truncate(expected_len);
    Ok(output)
}

fn deinterlace(indices: Vec<u8>, width: usize, height: usize) -> Result<Vec<u8>, &'static str> {
    if indices.len() < width * height {
        return Err("interlace buffer too small");
    }

    let mut out = vec![0u8; width * height];
    let mut src = 0usize;
    for (start, step) in [(0usize, 8usize), (4, 8), (2, 4), (1, 2)] {
        let mut y = start;
        while y < height {
            let end = src + width;
            if end > indices.len() {
                return Err("interlace source overflow");
            }
            let dst = y * width;
            out[dst..dst + width].copy_from_slice(&indices[src..end]);
            src = end;
            y += step;
        }
    }
    Ok(out)
}
