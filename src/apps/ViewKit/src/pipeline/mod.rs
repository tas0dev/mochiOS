pub mod display_list;
pub mod dom;
pub mod framebuffer;
pub mod image;
pub mod layout;
pub mod parser;
pub mod rasterizer;
pub mod style;

use display_list::DisplayList;
use framebuffer::Framebuffer;
use layout::LayoutTree;
use style::StyledTree;

pub struct RenderOutput {
    pub dom: dom::DomDocument,
    pub styled: StyledTree,
    pub layout: LayoutTree,
    pub display_list: DisplayList,
    pub framebuffer: Framebuffer,
}

pub fn render_document(html: &str, css: &str, width: u32, height: u32) -> RenderOutput {
    let parsed = parser::parse(html, css);
    let styled = style::compute_styles(&parsed.dom, &parsed.stylesheet);
    let layout = layout::compute_layout(&styled, width, height);
    let display_list = display_list::build(&layout);
    let framebuffer = rasterizer::rasterize(&display_list, width, height);

    RenderOutput {
        dom: parsed.dom,
        styled,
        layout,
        display_list,
        framebuffer,
    }
}
