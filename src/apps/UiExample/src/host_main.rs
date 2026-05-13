use viewkit::{AppBuilder, VComponent};
use viewkit::components::{card, text};

pub fn main() -> Result<(), String> {
    const WIDTH: u32 = 960;
    const HEIGHT: u32 = 540;

    AppBuilder::new(WIDTH, HEIGHT)
        .children(|| view())?
        .build()?
        .run()
}

fn view() -> VComponent {
    VComponent::from_str(
        "<style>
            .root{width:100%;height:100%;background:#f4f7fa;padding:18px;gap:12px;display:flex;flex-direction:column;box-sizing:border-box;}
            .title{font-size:20px;}
        </style>
        <div class=\"root\"><Children /></div>",
    )
    .child(card().children([
        VComponent::from_str("<div class=\"title\"><Children /></div>").child(text().text("UiExample (host)".to_string())),
        text().text("This is a host-only preview. On mochiOS: U cycles pages.".to_string()),
    ]))
}
