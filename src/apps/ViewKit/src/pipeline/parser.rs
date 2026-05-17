use std::collections::BTreeMap;

use super::dom::{DomDocument, DomNode, DomNodeKind};

#[derive(Debug, Clone)]
pub struct Stylesheet {
    pub rules: Vec<StyleRule>,
}

#[derive(Debug, Clone)]
pub struct StyleRule {
    pub selector: String,
    pub declarations: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct ParsedDocument {
    pub dom: DomDocument,
    pub stylesheet: Stylesheet,
}

pub fn parse(html: &str, css: &str) -> ParsedDocument {
    ParsedDocument {
        dom: parse_html(html),
        stylesheet: parse_css(css),
    }
}

fn parse_html(html: &str) -> DomDocument {
    let mut root = DomNode::element("document");
    let mut stack: Vec<DomNode> = Vec::new();
    stack.push(DomNode::element("body"));

    let mut rest = html;
    while !rest.is_empty() {
        if let Some(start) = rest.find('<') {
            if start > 0 {
                let text = rest[..start].trim();
                if !text.is_empty() {
                    push_child(&mut stack, DomNode::text(text));
                }
            }

            let after_lt = &rest[start + 1..];
            if let Some(end) = after_lt.find('>') {
                let raw_tag = after_lt[..end].trim();
                rest = &after_lt[end + 1..];

                if raw_tag.starts_with('!') {
                    continue;
                }

                if raw_tag.starts_with('/') {
                    if stack.len() > 1 {
                        let node = stack.pop().expect("stack is non-empty");
                        push_child(&mut stack, node);
                    }
                    continue;
                }

                let self_closing = raw_tag.ends_with('/');
                let raw_tag = raw_tag.trim_end_matches('/').trim();
                let tag_name = raw_tag
                    .trim_end_matches('/')
                    .split_whitespace()
                    .next()
                    .unwrap_or("div")
                    .to_ascii_lowercase();
                let mut node = DomNode::element(tag_name);
                parse_attributes(raw_tag, &mut node);

                if self_closing {
                    push_child(&mut stack, node);
                } else {
                    stack.push(node);
                }
            } else {
                let text = rest.trim();
                if !text.is_empty() {
                    push_child(&mut stack, DomNode::text(text));
                }
                break;
            }
        } else {
            let text = rest.trim();
            if !text.is_empty() {
                push_child(&mut stack, DomNode::text(text));
            }
            break;
        }
    }

    while stack.len() > 1 {
        let node = stack.pop().expect("stack is non-empty");
        push_child(&mut stack, node);
    }

    if let Some(body) = stack.pop() {
        root.children.push(body);
    }

    DomDocument { root }
}

fn push_child(stack: &mut [DomNode], child: DomNode) {
    if let Some(last) = stack.last_mut() {
        last.children.push(child);
    }
}

fn parse_attributes(raw_tag: &str, node: &mut DomNode) {
    let mut parts = raw_tag.split_whitespace();
    let _tag_name = parts.next();
    if let DomNodeKind::Element(element) = &mut node.kind {
        for attr in parts {
            if let Some((key, value)) = attr.split_once('=') {
                let value = value.trim_matches('"').trim_matches('\'').to_string();
                element
                    .attributes
                    .insert(key.to_ascii_lowercase(), value);
            }
        }
    }
}

fn parse_css(css: &str) -> Stylesheet {
    let mut rules = Vec::new();
    let mut rest = css;

    while let Some(open) = rest.find('{') {
        let selector = rest[..open].trim();
        let after_open = &rest[open + 1..];
        let Some(close) = after_open.find('}') else {
            break;
        };
        let body = &after_open[..close];
        rest = &after_open[close + 1..];

        if selector.is_empty() {
            continue;
        }

        let mut declarations = BTreeMap::new();
        for declaration in body.split(';') {
            let declaration = declaration.trim();
            if declaration.is_empty() {
                continue;
            }
            if let Some((key, value)) = declaration.split_once(':') {
                declarations.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }

        rules.push(StyleRule {
            selector: selector.to_ascii_lowercase(),
            declarations,
        });
    }

    Stylesheet { rules }
}

pub fn pretty_print_dom(node: &DomNode, depth: usize, out: &mut String) {
    let indent = "  ".repeat(depth);
    match &node.kind {
        DomNodeKind::Element(el) => {
            out.push_str(&format!("{indent}<{}>\n", el.tag_name));
            for child in &node.children {
                pretty_print_dom(child, depth + 1, out);
            }
            out.push_str(&format!("{indent}</{}>\n", el.tag_name));
        }
        DomNodeKind::Text(text) => {
            out.push_str(&format!("{indent}\"{}\"\n", text));
        }
    }
}
