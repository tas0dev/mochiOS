use crate::components::VComponent;
use crate::{host_HostDisplay, host_HostSurface, pipeline, register_pointer_and_keyboard};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub type UIBuilder = Box<dyn Fn() -> VComponent + Send + Sync>;

pub struct AppBuilder {
    width: u32,
    height: u32,
    ui_fn: Option<UIBuilder>,
}

pub struct App {
    host: host_HostDisplay,
    surface: host_HostSurface,
    ui_fn: UIBuilder,
    width: u32,
    height: u32,
}

impl AppBuilder {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            ui_fn: None,
        }
    }

    /// UIビルダー関数を設定（毎フレーム呼び出される）
    pub fn children<F>(mut self, ui_fn: F) -> Result<Self, String>
    where
        F: Fn() -> VComponent + Send + Sync + 'static,
    {
        self.ui_fn = Some(Box::new(ui_fn));
        Ok(self)
    }

    pub fn build(self) -> Result<App, String> {
        let mut host = host_HostDisplay::new()?;
        let mut surface = host.create_surface(self.width as i32, self.height as i32)?;
        host.set_toplevel(&mut surface)?;

        Ok(App {
            host,
            surface,
            ui_fn: self.ui_fn.ok_or("UI function not set".to_string())?,
            width: self.width,
            height: self.height,
        })
    }
}

impl App {
    pub fn new(width: u32, height: u32) -> AppBuilder {
        AppBuilder::new(width, height)
    }

    pub fn run(mut self) -> Result<(), String> {
        register_input_handlers(&mut self.host)?;

        let frame_done = Arc::new(AtomicBool::new(false));
        let mut frame_count = 0_u32;

        loop {
            // 毎フレーム UIビルダー関数を呼び出してUIを再構築
            let ui = (self.ui_fn)();
            let html = ui.render();
            let css = ui.css();

            let rendered = pipeline::render_document(&html, &css, self.width, self.height);
            blit_framebuffer_to_surface(&rendered.framebuffer.pixels, self.surface.back_buffer_mut());
            self.surface.swap_and_commit()?;

            frame_done.store(false, Ordering::SeqCst);
            self.surface.request_frame(frame_done.clone())?;
            self.surface.commit_front()?;

            while !frame_done.load(Ordering::SeqCst) {
                self.host.dispatch()?;
                idle_wait();
            }

            frame_count += 1;
            if frame_count % 120 == 0 {
                println!("app: frame {}", frame_count);
            }
        }
    }
}

fn register_input_handlers(host: &mut host_HostDisplay) -> Result<(), String> {
    register_pointer_and_keyboard(host, None, None)
}

fn idle_wait() {
    for _ in 0..50 {
        core::hint::spin_loop();
    }
}

fn blit_framebuffer_to_surface(src_argb: &[u32], dst: &mut [u8]) {
    let pixel_count = src_argb.len().min(dst.len() / 4);
    for i in 0..pixel_count {
        let argb = src_argb[i];
        let bytes = argb.to_ne_bytes();
        let base = i * 4;
        dst[base] = bytes[0];
        dst[base + 1] = bytes[1];
        dst[base + 2] = bytes[2];
        dst[base + 3] = 0x00;
    }
}
