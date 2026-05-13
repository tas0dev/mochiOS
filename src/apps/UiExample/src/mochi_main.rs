use viewkit::{AppControl, AppEvent, AppRunner, Redraw, VComponent};
use viewkit::components::{button, card, text};

const WIDTH: u16 = 960;
const HEIGHT: u16 = 540;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    Typography = 0,
    Layout = 1,
    Opacity = 2,
}

struct Model {
    page: Page,
}

pub fn main() {
    println!("[UiExample] start (ESC exit, U switch page)");

    let runner = match AppRunner::new(WIDTH, HEIGHT) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[UiExample] AppRunner init failed: {}", e);
            return;
        }
    };

    let model = Model {
        page: Page::Typography,
    };

    let res = runner.run(model, view, update);
    if let Err(e) = res {
        eprintln!("[UiExample] run failed: {}", e);
    }
}

fn view(model: &Model) -> VComponent {
    match model.page {
        Page::Typography => typography_page(),
        Page::Layout => layout_page(),
        Page::Opacity => opacity_page(),
    }
}

fn update(model: &mut Model, ev: AppEvent) -> (AppControl, Redraw) {
    match ev {
        AppEvent::KeyScancode(sc) => {
            // ESC
            if sc == 0x01 || sc == 0x81 {
                return (AppControl::Exit, Redraw::No);
            }

            // U press: cycle pages
            if sc == 0x16 {
                model.page = match model.page {
                    Page::Typography => Page::Layout,
                    Page::Layout => Page::Opacity,
                    Page::Opacity => Page::Typography,
                };
                return (AppControl::Continue, Redraw::Yes);
            }
        }
        AppEvent::Mouse(_) => {}
    }

    (AppControl::Continue, Redraw::No)
}

fn typography_page() -> VComponent {
    root().children([
        header("Typography / TrueType (NotoSansJP)"),
        sized_text(24.0, "日本語テキスト: こんにちは世界"),
        sized_text(16.0, "ASCII: The quick brown fox jumps over the lazy dog."),
        sized_text(12.0, "Mixed: Rust🦀 + 日本語 + 12345"),
        hint("Keys: U=next page, ESC=exit"),
    ])
}

fn layout_page() -> VComponent {
    root().child(row().children([
        card().children([
            header("Layout (flex / gap / padding)"),
            text().text("left panel".to_string()),
            sized_text(12.0, "padding / border-radius / background"),
            button().child(text().text("button()".to_string())),
        ]),
        card().child(text().text("Right panel (日本語もOK)".to_string())),
    ]))
}

fn opacity_page() -> VComponent {
    root().child(row().children([
        opacity_wrap(1.0, card().children([header("Opacity 1.0"), text().text("背景 + テキスト".to_string())])),
        opacity_wrap(0.5, card().children([header("Opacity 0.5"), text().text("半透明合成".to_string())])),
        opacity_wrap(0.25, card().children([header("Opacity 0.25"), text().text("テキストも半透明".to_string())])),
    ]))
}

fn root() -> VComponent {
    VComponent::from_str(
        "<style>
            .root{width:100%;height:100%;background:#f4f7fa;padding:18px;gap:12px;display:flex;flex-direction:column;box-sizing:border-box;}
            .row{display:flex;flex-direction:row;gap:12px;width:100%;box-sizing:border-box;}
            .hint .text{color:#334155;}
        </style>
        <div class=\"root\"><Children /></div>",
    )
}

fn row() -> VComponent {
    VComponent::from_str("<div class=\"row\"><Children /></div>")
}

fn header(s: &str) -> VComponent {
    sized_text(20.0, s)
}

fn hint(s: &str) -> VComponent {
    VComponent::from_str("<div class=\"hint\"><Children /></div>").child(text().text(s.to_string()))
}

fn sized_text(px: f32, s: &str) -> VComponent {
    // font-size is parsed/handled by ViewKit style now.
    let wrapper = if (px - 24.0).abs() < f32::EPSILON {
        FS24
    } else if (px - 20.0).abs() < f32::EPSILON {
        FS20
    } else if (px - 12.0).abs() < f32::EPSILON {
        FS12
    } else {
        FS16
    };
    VComponent::from_str(wrapper).child(text().text(s.to_string()))
}

fn opacity_wrap(opacity: f32, child: VComponent) -> VComponent {
    let wrapper = if (opacity - 0.25).abs() < f32::EPSILON {
        OP25
    } else if (opacity - 0.5).abs() < f32::EPSILON {
        OP50
    } else {
        OP100
    };
    VComponent::from_str(wrapper).child(child)
}

const FS24: &str = "<style>.fs{font-size:24px;}</style><div class=\"fs\"><Children /></div>";
const FS20: &str = "<style>.fs{font-size:20px;}</style><div class=\"fs\"><Children /></div>";
const FS16: &str = "<style>.fs{font-size:16px;}</style><div class=\"fs\"><Children /></div>";
const FS12: &str = "<style>.fs{font-size:12px;}</style><div class=\"fs\"><Children /></div>";

const OP100: &str = "<style>.o{opacity:1;}</style><div class=\"o\"><Children /></div>";
const OP50: &str = "<style>.o{opacity:0.5;}</style><div class=\"o\"><Children /></div>";
const OP25: &str = "<style>.o{opacity:0.25;}</style><div class=\"o\"><Children /></div>";
