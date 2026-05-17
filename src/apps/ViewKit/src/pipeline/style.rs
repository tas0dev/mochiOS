use std::collections::BTreeMap;

use super::dom::{DomDocument, DomNode, DomNodeKind, ElementData};
use super::parser::Stylesheet;

pub type StyleMap = BTreeMap<String, String>;

#[derive(Debug, Clone)]
pub struct StyledTree {
    pub root: StyledNode,
}

#[derive(Debug, Clone)]
pub struct StyledNode {
    pub node: DomNode,
    pub styles: StyleMap,
    pub children: Vec<StyledNode>,
}

pub fn compute_styles(dom: &DomDocument, stylesheet: &Stylesheet) -> StyledTree {
    StyledTree {
        root: style_node(&dom.root, stylesheet, None),
    }
}

fn style_node(node: &DomNode, stylesheet: &Stylesheet, inherited_color: Option<&str>) -> StyledNode {
    let mut styles = BTreeMap::new();

    if let DomNodeKind::Element(element) = &node.kind {
        for rule in &stylesheet.rules {
            if selector_matches(&rule.selector, element) {
                styles.extend(rule.declarations.clone());
            }
        }
    }

    if !styles.contains_key("color") {
        if let Some(color) = inherited_color {
            styles.insert("color".to_string(), color.to_string());
        }
    }

    let next_inherited_color = styles
        .get("color")
        .map(String::as_str)
        .or(inherited_color);

    let children = node
        .children
        .iter()
        .map(|child| style_node(child, stylesheet, next_inherited_color))
        .collect();

    StyledNode {
        node: node.clone(),
        styles,
        children,
    }
}

fn selector_matches(selector: &str, element: &ElementData) -> bool {
    selector
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .any(|simple| simple_selector_matches(simple, element))
}

fn simple_selector_matches(selector: &str, element: &ElementData) -> bool {
    if selector == "*" {
        return true;
    }

    if let Some(id_sel) = selector.strip_prefix('#') {
        return element
            .attributes
            .get("id")
            .map(|id| id == id_sel)
            .unwrap_or(false);
    }

    if let Some(class_sel) = selector.strip_prefix('.') {
        return has_class(element, class_sel);
    }

    if let Some((tag, class_sel)) = selector.split_once('.') {
        return element.tag_name == tag && has_class(element, class_sel);
    }

    if let Some((tag, id_sel)) = selector.split_once('#') {
        return element.tag_name == tag
            && element
                .attributes
                .get("id")
                .map(|id| id == id_sel)
                .unwrap_or(false);
    }

    element.tag_name == selector
}

fn has_class(element: &ElementData, class_name: &str) -> bool {
    element
        .attributes
        .get("class")
        .map(|classes| classes.split_whitespace().any(|c| c == class_name))
        .unwrap_or(false)
}
