//! App-side runner for mochiOS apps.
//!
#![cfg(all(target_os = "linux", target_env = "musl"))]

use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use swiftlib::{
    keyboard::{read_scancode, read_scancode_tap},
    mouse::{self, MousePacket},
    task::yield_now,
};

use crate::{ipc_proto, render_component_to_pixmap, VComponent, Window};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppEvent {
    KeyScancode(u8),
    Mouse(MousePacket),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppControl {
    Continue,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Redraw {
    No,
    Yes,
}

pub struct AppRunner {
    window: Window,
    dirty: Arc<AtomicBool>,
}

impl AppRunner {
    pub fn new(width: u16, height: u16) -> Result<Self, &'static str> {
        Self::new_with_layer(width, height, ipc_proto::LAYER_APP)
    }

    pub fn new_with_layer(width: u16, height: u16, layer: u8) -> Result<Self, &'static str> {
        Ok(Self {
            window: Window::new(width, height, layer)?,
            dirty: Arc::new(AtomicBool::new(true)),
        })
    }

    /// Mark the next loop iteration as needing a redraw.
    pub fn request_redraw(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    /// Attach a ViewKit `State<T>` so changes trigger redraw automatically.
    pub fn watch_state<T: Clone + Send + Sync + 'static>(&self, state: &crate::State<T>) {
        let dirty = self.dirty.clone();
        state.on_change(Box::new(move || {
            dirty.store(true, Ordering::SeqCst);
        }));
    }

    /// Poll one input event (non-blocking).
    fn poll_event() -> Option<AppEvent> {
        // Prefer tap queue when available so we don't fight with shell.service.
        let sc_opt = match read_scancode_tap() {
            Ok(Some(sc)) => Some(sc),
            Ok(None) => read_scancode(),
            Err(_) => read_scancode(),
        };
        if let Some(sc) = sc_opt {
            return Some(AppEvent::KeyScancode(sc));
        }

        match mouse::read_packet() {
            Ok(Some(pkt)) => Some(AppEvent::Mouse(pkt)),
            _ => None,
        }
    }

    pub fn run<M, ViewFn, UpdateFn>(
        mut self,
        mut model: M,
        view: ViewFn,
        mut update: UpdateFn,
    ) -> Result<(), &'static str>
    where
        ViewFn: Fn(&M) -> VComponent,
        UpdateFn: FnMut(&mut M, AppEvent) -> (AppControl, Redraw),
    {
        let (w, h) = self.window.size();
        let width = w as usize;
        let height = h as usize;

        loop {
            // Drain input queue quickly; update may request redraw.
            while let Some(ev) = Self::poll_event() {
                let (ctl, redraw) = update(&mut model, ev);
                if redraw == Redraw::Yes {
                    self.request_redraw();
                }
                if ctl == AppControl::Exit {
                    return Ok(());
                }
            }

            if self.dirty.swap(false, Ordering::SeqCst) {
                let ui = view(&model);
                let pixels = render_component_to_pixmap(&ui, width as u32, height as u32);
                self.window.present(&pixels)?;
            }

            yield_now();
        }
    }
}

