// ViewKit のホスト向け Shim を公開する
// Shared libkagami is hosted under ../Kagami.
#[path = "../../Kagami/src/libkagami.rs"]
pub mod libkagami;
pub mod pipeline;
pub mod components;
pub mod app;
pub mod state;

pub use libkagami::*;
pub use app::AppBuilder;
pub use state::State;
pub use components::VComponent;

#[cfg(all(target_os = "linux", target_env = "musl"))]
pub mod app_runner;
#[cfg(all(target_os = "linux", target_env = "musl"))]
pub use app_runner::{AppControl, AppEvent, AppRunner, Redraw};

// mochiOS アプリターゲット (x86_64-unknown-linux-musl / no-default-libraries) では
// Kagami IPC/共有面を ViewKit 側で面倒見る。
#[cfg(all(target_os = "linux", target_env = "musl"))]
#[path = "../../Kagami/src/ipc_proto.rs"]
pub mod ipc_proto;

#[cfg(all(target_os = "linux", target_env = "musl"))]
pub mod window;

#[cfg(all(target_os = "linux", target_env = "musl"))]
pub use window::Window;

/// VComponent を pixel buffer に render
pub fn render_component_to_pixmap(component: &VComponent, width: u32, height: u32) -> Vec<u32> {
    let html = component.render();
    let css = component.css();
    let output = pipeline::render_document(&html, &css, width, height);
    output.framebuffer.pixels
}
