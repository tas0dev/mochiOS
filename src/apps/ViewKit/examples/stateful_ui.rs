use viewkit::components::VComponent;
use viewkit::components_list;
use viewkit::AppBuilder;
use std::sync::Arc;
use crate::State::{Detail, Home};

components_list! {
    button,
    card,
    text,
}

#[repr(i32)]
enum State {
    Home = 0i32,
    Detail = 1i32,
}

#[cfg(unix)]
fn main() -> Result<(), String> {
    const WIDTH: u32 = 960;
    const HEIGHT: u32 = 540;

    // 画面状態を管理
    let screen_state: Arc<viewkit::State<i32>> = Arc::new(viewkit::State::new(Home as i32));

    AppBuilder::new(WIDTH, HEIGHT)
        .children({
            let state = screen_state.clone();
            move || {
                let current_screen = state.get();

                if current_screen == 0 {
                    // ホーム画面
                    let state = state.clone();
                    card()
                        .label("Home Screen - Click to Detail")
                        .on_click(move || {
                            state.set(Detail as i32);
                            println!("Navigated to detail screen");
                        })
                } else {
                    // 詳細画面
                    let state = state.clone();
                    card()
                        .label("Detail Screen - Click to Home")
                        .on_click(move || {
                            state.set(Home as i32);
                            println!("Navigated back to home");
                        })
                }
            }
        })?
        .build()?
        .run()
}

#[cfg(not(unix))]
fn main() {
    eprintln!("stateful_ui requires a unix host with Wayland.");
}
