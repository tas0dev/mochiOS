use super::dom::DomNodeKind;
use super::style::{StyleMap, StyledNode, StyledTree};
use std::collections::BTreeMap;
use ui_layout::{
    AlignItems, BoxSizing, Display, FlexDirection, Fragment, ItemFragment, ItemStyle,
    JustifyContent, LayoutBoxes, LayoutEngine, LayoutNode as UiLayoutNode, Length, SizeStyle,
    Spacing, Style as UiStyle,
};

#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

#[derive(Debug, Clone)]
pub struct LayoutTree {
    pub root: LayoutNode,
}

#[derive(Debug, Clone)]
pub struct LayoutNode {
    pub rect: Rect,
    pub styles: StyleMap,
    pub kind: LayoutNodeKind,
    pub children: Vec<LayoutNode>,
}

#[derive(Debug, Clone)]
pub enum LayoutNodeKind {
    Element { tag_name: String, attributes: BTreeMap<String, String> },
    Text { content: String },
}

#[derive(Debug, Clone)]
struct MetaNode {
    kind: LayoutNodeKind,
    styles: StyleMap,
    children: Vec<MetaNode>,
}

pub fn compute_layout(styled: &StyledTree, width: u32, height: u32) -> LayoutTree {
    let (mut ui_root, meta_root) = build_ui_tree(&styled.root, true);
    LayoutEngine::layout(&mut ui_root, width as f32, height as f32);

    let root = to_layout_tree(&ui_root, &meta_root, (0.0, 0.0));
    LayoutTree { root }
}

fn build_ui_tree(node: &StyledNode, is_root: bool) -> (UiLayoutNode, MetaNode) {
    match &node.node.kind {
        DomNodeKind::Element(el) => {
            let mut style = style_from_css(&node.styles);
            if is_root {
                style.size.width = Length::Percent(100.0);
                style.size.height = Length::Percent(100.0);
            }

            let mut ui_children = Vec::with_capacity(node.children.len());
            let mut meta_children = Vec::with_capacity(node.children.len());
            for child in &node.children {
                let (ui_child, meta_child) = build_ui_tree(child, false);
                ui_children.push(ui_child);
                meta_children.push(meta_child);
            }

            let ui_node = UiLayoutNode::with_children(style, ui_children);
            let meta = MetaNode {
                kind: LayoutNodeKind::Element {
                    tag_name: el.tag_name.clone(),
                    attributes: el.attributes.clone(),
                },
                styles: node.styles.clone(),
                children: meta_children,
            };
            (ui_node, meta)
        }
        DomNodeKind::Text(text) => {
            let mut ui_node = UiLayoutNode::new(UiStyle {
                display: Display::Inline,
                ..Default::default()
            });
            ui_node.set_fragments(vec![ItemFragment::Fragment(Fragment {
                width: estimate_text_width(text),
                height: estimate_text_height(text),
            })]);

            let meta = MetaNode {
                kind: LayoutNodeKind::Text {
                    content: text.clone(),
                },
                styles: node.styles.clone(),
                children: Vec::new(),
            };
            (ui_node, meta)
        }
    }
}

fn to_layout_tree(ui: &UiLayoutNode, meta: &MetaNode, parent_content_abs: (f32, f32)) -> LayoutNode {
    let (border_x, border_y, border_w, border_h, content_abs) = match &ui.layout_boxes {
        LayoutBoxes::Single(model) => {
            let border_abs_x = parent_content_abs.0 + model.border_box.x;
            let border_abs_y = parent_content_abs.1 + model.border_box.y;
            let content_abs_x = parent_content_abs.0 + model.content_box.x;
            let content_abs_y = parent_content_abs.1 + model.content_box.y;
            (
                border_abs_x,
                border_abs_y,
                model.border_box.width,
                model.border_box.height,
                (content_abs_x, content_abs_y),
            )
        }
        _ => (0.0, 0.0, 0.0, 0.0, parent_content_abs),
    };

    let children = ui
        .children
        .iter()
        .zip(meta.children.iter())
        .map(|(child_ui, child_meta)| to_layout_tree(child_ui, child_meta, content_abs))
        .collect();

    LayoutNode {
        rect: Rect {
            x: border_x.round() as i32,
            y: border_y.round() as i32,
            width: border_w.max(0.0).round() as i32,
            height: border_h.max(0.0).round() as i32,
        },
        styles: meta.styles.clone(),
        kind: meta.kind.clone(),
        children,
    }
}

fn style_from_css(styles: &StyleMap) -> UiStyle {
    let mut style = UiStyle::default();

    if let Some(display) = styles.get("display") {
        style.display = match display.trim() {
            "flex" => Display::Flex {
                flex_direction: parse_flex_direction(styles.get("flex-direction")),
            },
            "inline" => Display::Inline,
            "none" => Display::None,
            _ => Display::Block,
        };
    }

    style.justify_content = parse_justify_content(styles.get("justify-content"));
    style.align_items = parse_align_items(styles.get("align-items"));
    style.column_gap = parse_length(styles.get("column-gap"), Length::Px(0.0));
    style.row_gap = parse_length(styles.get("row-gap"), Length::Px(0.0));
    if let Some(gap) = styles.get("gap") {
        let g = parse_length_token(gap).unwrap_or(Length::Px(0.0));
        style.column_gap = g.clone();
        style.row_gap = g;
    }

    style.size = SizeStyle {
        width: parse_length(styles.get("width"), Length::Auto),
        height: parse_length(styles.get("height"), Length::Auto),
        min_width: parse_length(styles.get("min-width"), Length::Auto),
        max_width: parse_length(styles.get("max-width"), Length::Auto),
        min_height: parse_length(styles.get("min-height"), Length::Auto),
        max_height: parse_length(styles.get("max-height"), Length::Auto),
    };

    style.item_style = ItemStyle {
        flex_grow: parse_f32(styles.get("flex-grow"), 0.0),
        flex_shrink: parse_f32(styles.get("flex-shrink"), 1.0),
        flex_basis: parse_length(styles.get("flex-basis"), Length::Auto),
        align_self: parse_align_self(styles.get("align-self")),
    };

    style.box_sizing = match styles.get("box-sizing").map(|s| s.trim()) {
        Some("border-box") => BoxSizing::BorderBox,
        _ => BoxSizing::ContentBox,
    };

    style.spacing = spacing_from_css(styles);

    style
}

fn parse_flex_direction(direction: Option<&String>) -> FlexDirection {
    match direction.map(|v| v.trim()) {
        Some("row") => FlexDirection::Row,
        _ => FlexDirection::Column,
    }
}

fn parse_length(value: Option<&String>, default: Length) -> Length {
    let Some(raw) = value else {
        return default;
    };
    parse_length_token(raw).unwrap_or(default)
}

fn parse_length_token(raw: &str) -> Option<Length> {
    let s = raw.trim().to_ascii_lowercase();

    if s == "auto" {
        return Some(Length::Auto);
    }
    if let Some(px) = s.strip_suffix("px") {
        return px
            .trim()
            .parse::<f32>()
            .map(Length::Px)
            .ok();
    }
    if let Some(pct) = s.strip_suffix('%') {
        return pct
            .trim()
            .parse::<f32>()
            .map(Length::Percent)
            .ok();
    }
    if let Some(vw) = s.strip_suffix("vw") {
        return vw
            .trim()
            .parse::<f32>()
            .map(Length::Vw)
            .ok();
    }
    if let Some(vh) = s.strip_suffix("vh") {
        return vh
            .trim()
            .parse::<f32>()
            .map(Length::Vh)
            .ok();
    }

    s.parse::<f32>().map(Length::Px).ok()
}

fn parse_f32(v: Option<&String>, default: f32) -> f32 {
    v.and_then(|s| s.trim().parse::<f32>().ok()).unwrap_or(default)
}

fn parse_justify_content(value: Option<&String>) -> JustifyContent {
    match value.map(|s| s.trim()) {
        Some("center") => JustifyContent::Center,
        Some("end") | Some("flex-end") => JustifyContent::End,
        Some("space-between") => JustifyContent::SpaceBetween,
        Some("space-around") => JustifyContent::SpaceAround,
        Some("space-evenly") => JustifyContent::SpaceEvenly,
        _ => JustifyContent::Start,
    }
}

fn parse_align_items(value: Option<&String>) -> AlignItems {
    match value.map(|s| s.trim()) {
        Some("start") | Some("flex-start") => AlignItems::Start,
        Some("center") => AlignItems::Center,
        Some("end") | Some("flex-end") => AlignItems::End,
        _ => AlignItems::Stretch,
    }
}

fn parse_align_self(value: Option<&String>) -> Option<AlignItems> {
    match value.map(|s| s.trim()) {
        Some("auto") | None => None,
        Some("start") | Some("flex-start") => Some(AlignItems::Start),
        Some("center") => Some(AlignItems::Center),
        Some("end") | Some("flex-end") => Some(AlignItems::End),
        Some("stretch") => Some(AlignItems::Stretch),
        _ => None,
    }
}

fn spacing_from_css(styles: &StyleMap) -> Spacing {
    let mut spacing = Spacing::default();

    if let Some(border) = styles.get("border") {
        if let Some(width) = border.split_whitespace().find_map(parse_length_token) {
            spacing.border_top = width.clone();
            spacing.border_right = width.clone();
            spacing.border_bottom = width.clone();
            spacing.border_left = width;
        }
    }

    if let Some(sh) = parse_quad_shorthand(styles.get("margin")) {
        spacing.margin_top = sh[0].clone();
        spacing.margin_right = sh[1].clone();
        spacing.margin_bottom = sh[2].clone();
        spacing.margin_left = sh[3].clone();
    }
    if let Some(sh) = parse_quad_shorthand(styles.get("padding")) {
        spacing.padding_top = sh[0].clone();
        spacing.padding_right = sh[1].clone();
        spacing.padding_bottom = sh[2].clone();
        spacing.padding_left = sh[3].clone();
    }
    if let Some(sh) = parse_quad_shorthand(styles.get("border-width")) {
        spacing.border_top = sh[0].clone();
        spacing.border_right = sh[1].clone();
        spacing.border_bottom = sh[2].clone();
        spacing.border_left = sh[3].clone();
    }

    spacing.margin_top = parse_length(styles.get("margin-top"), spacing.margin_top);
    spacing.margin_right = parse_length(styles.get("margin-right"), spacing.margin_right);
    spacing.margin_bottom = parse_length(styles.get("margin-bottom"), spacing.margin_bottom);
    spacing.margin_left = parse_length(styles.get("margin-left"), spacing.margin_left);

    spacing.padding_top = parse_length(styles.get("padding-top"), spacing.padding_top);
    spacing.padding_right = parse_length(styles.get("padding-right"), spacing.padding_right);
    spacing.padding_bottom = parse_length(styles.get("padding-bottom"), spacing.padding_bottom);
    spacing.padding_left = parse_length(styles.get("padding-left"), spacing.padding_left);

    spacing.border_top = parse_length(styles.get("border-top-width"), spacing.border_top);
    spacing.border_right = parse_length(styles.get("border-right-width"), spacing.border_right);
    spacing.border_bottom = parse_length(styles.get("border-bottom-width"), spacing.border_bottom);
    spacing.border_left = parse_length(styles.get("border-left-width"), spacing.border_left);

    spacing
}

fn parse_quad_shorthand(value: Option<&String>) -> Option<[Length; 4]> {
    let raw = value?;
    let tokens: Vec<_> = raw
        .split_whitespace()
        .filter_map(parse_length_token)
        .collect();
    match tokens.len() {
        1 => Some([
            tokens[0].clone(),
            tokens[0].clone(),
            tokens[0].clone(),
            tokens[0].clone(),
        ]),
        2 => Some([
            tokens[0].clone(),
            tokens[1].clone(),
            tokens[0].clone(),
            tokens[1].clone(),
        ]),
        3 => Some([
            tokens[0].clone(),
            tokens[1].clone(),
            tokens[2].clone(),
            tokens[1].clone(),
        ]),
        4 => Some([
            tokens[0].clone(),
            tokens[1].clone(),
            tokens[2].clone(),
            tokens[3].clone(),
        ]),
        _ => None,
    }
}

fn estimate_text_width(text: &str) -> f32 {
    (text.chars().count() as f32 * 8.0).max(8.0)
}

fn estimate_text_height(_text: &str) -> f32 {
    16.0
}
