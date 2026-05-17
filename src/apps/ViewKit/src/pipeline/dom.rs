use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct DomDocument {
    pub root: DomNode,
}

#[derive(Debug, Clone)]
pub struct DomNode {
    pub kind: DomNodeKind,
    pub children: Vec<DomNode>,
}

#[derive(Debug, Clone)]
pub enum DomNodeKind {
    Element(ElementData),
    Text(String),
}

#[derive(Debug, Clone)]
pub struct ElementData {
    pub tag_name: String,
    pub attributes: BTreeMap<String, String>,
}

impl DomNode {
    pub fn element(tag_name: impl Into<String>) -> Self {
        Self {
            kind: DomNodeKind::Element(ElementData {
                tag_name: tag_name.into(),
                attributes: BTreeMap::new(),
            }),
            children: Vec::new(),
        }
    }

    pub fn text(content: impl Into<String>) -> Self {
        Self {
            kind: DomNodeKind::Text(content.into()),
            children: Vec::new(),
        }
    }
}
