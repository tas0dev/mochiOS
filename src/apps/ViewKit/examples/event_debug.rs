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

    println!("ViewKit Event Debug Test");
    println!("Move your mouse and click to see logs");
    println!();

    AppBuilder::new(WIDTH, HEIGHT)
        .children(|| {
            card()
                .label("Mouse & Keyboard Test\n\nMove mouse or press keys\nCheck console for logs")
        })?
        .build()?
        .run()
}

#[cfg(not(unix))]
fn main() {
    eprintln!("event_debug requires a unix host with Wayland.");
}
