use viewkit::components::VComponent;
use viewkit::components_list;
use viewkit::AppBuilder;

components_list! {
    button,
    card,
    text,
}

#[cfg(unix)]
fn main() -> Result<(), String> {
    const WIDTH: u32 = 960;
    const HEIGHT: u32 = 540;

    AppBuilder::new(WIDTH, HEIGHT)
        .children(|| {
            let icons = (0..5).map(|_| card());
            card().children(icons)
        })?
        .build()?
        .run()
}

#[cfg(not(unix))]
fn main() {
    eprintln!("ui_test requires a unix host with Wayland.");
}
