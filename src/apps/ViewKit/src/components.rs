use std::collections::HashMap;

// イベントハンドラーの型
pub type EventHandler = Box<dyn Fn() + Send + Sync>;

// コンポーネントを表す構造体
pub struct VComponent {
    cached_html: String,
    cached_css: String,
    children: Vec<VComponent>,
    content: Vec<VContent>,
    attributes: HashMap<String, String>,
    handlers: HashMap<String, EventHandler>,
    visible: bool,
}

// TODO: 画像対応
#[allow(unused)]
pub struct VContent {
    string: Option<String>,
    image_path: Option<String>,
    image_fit: Option<ImageFit>,
    image_clip_radius: Option<i32>,
}

#[derive(Clone, Copy)]
enum ImageFit {
    Contain,
    Cover,
}

impl VComponent {
    pub fn from_str(document: &'static str) -> Self {
        let (html, css) = split_embedded_style(document);

        Self {
            cached_html: html,
            cached_css: css,
            children: Vec::new(),
            content: Vec::new(),
            attributes: HashMap::new(),
            handlers: HashMap::new(),
            visible: true,
        }
    }

    // ビルダーメソッド群
    pub fn label(mut self, text: impl Into<String>) -> Self {
        self.attributes.insert("label".to_string(), text.into());
        self
    }

    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.attributes.insert("id".to_string(), id.into());
        self
    }

    pub fn class(mut self, cls: impl Into<String>) -> Self {
        self.attributes.insert("class".to_string(), cls.into());
        self
    }

    pub fn width(mut self, width: u32) -> Self {
        self.attributes.insert("width".to_string(), format!("{}px", width));
        self
    }

    pub fn height(mut self, height: u32) -> Self {
        self.attributes.insert("height".to_string(), format!("{}px", height));
        self
    }

    pub fn text(mut self, content: impl Into<String>) -> Self {
        self.content.push(VContent::string(content.into()));
        self
    }

    pub fn image(mut self, path: impl Into<String>) -> Self {
        self.content.push(VContent::image(path.into()));
        self
    }

    pub fn on_click(mut self, handler: impl Fn() + Send + Sync + 'static) -> Self {
        self.handlers.insert("click".to_string(), Box::new(handler));
        self
    }

    pub fn if_visible(mut self, visible: bool) -> Self {
        self.visible = visible;
        self
    }

    pub fn new(self) -> Self {
        self
    }

    pub fn child(mut self, component: VComponent) -> Self {
        self.children.push(component);
        self
    }

    pub fn children(mut self, components: impl IntoIterator<Item =VComponent>) -> Self {
        self.children.extend(components);
        self
    }

    pub fn render(&self) -> String {
        if !self.visible {
            return String::new();
        }

        let children_html: String = self
            .children
            .iter()
            .map(|c| c.render())
            .collect::<Vec<_>>()
            .join("\n");

        let mut html = self.cached_html
            .replace("<Children />", &children_html)
            .replace("<Children/>", &children_html)
            .replace("<Children></Children>", &children_html);

        // Content placeholders (e.g. <Content type="Image" />) are expanded here.
        // This keeps the HTML parser/pipeline generic, while allowing VComponent builder APIs.
        for item in &self.content {
            if let Some(path) = &item.image_path {
                html = replace_first_content_image(&html, path, item.image_fit, item.image_clip_radius);
            } else if let Some(text) = &item.string {
                let escaped = escape_text(text);
                html = replace_first_content_text(&html, &escaped);
            }
        }

        // 属性をHTMLに埋め込む
        for (key, value) in &self.attributes {
            html = html.replace(
                &format!("{{{{ {} }}}}", key),
                value
            );
        }

        html
    }

    pub fn css(&self) -> String {
        let self_css = replace_size_placeholders(
            &self.cached_css,
            self.attributes.get("width"),
            self.attributes.get("height"),
        );
        let mut all_css = vec![".vk-img{width:100%;height:100%;}".to_string(), self_css];
        for child in &self.children {
            let child_css = child.css();
            if !child_css.is_empty() {
                all_css.push(child_css);
            }
        }
        merge_css(&all_css.iter().map(|s| s.as_str()).collect::<Vec<_>>())
    }

    pub fn get_handler(&self, event: &str) -> Option<&EventHandler> {
        self.handlers.get(event)
    }

    pub fn has_handler(&self, event: &str) -> bool {
        self.handlers.contains_key(event)
    }

    pub fn trigger_handler(&self, event: &str) {
        if let Some(handler) = self.handlers.get(event) {
            handler();
        }
    }

    pub fn get_attributes(&self) -> &HashMap<String, String> {
        &self.attributes
    }
}

impl VContent {
    pub fn string(s: String) -> Self {
        Self {
            string: Some(s),
            image_path: None,
            image_fit: None,
            image_clip_radius: None,
        }
    }

    pub fn image(path: String) -> Self {
        Self {
            string: None,
            image_path: Some(path),
            image_fit: None,
            image_clip_radius: None,
        }
    }
}

#[macro_export]
macro_rules! components_list {
    ($($name:ident),* $(,)?) => {
        $(
            fn $name() -> VComponent {
                VComponent::from_str(include_str!(concat!(
                    "../resources/components/",
                    stringify!($name),
                    ".html"
                )))
            }
        )*
    };
}

fn split_embedded_style(document: &str) -> (String, String) {
    let open_tag = "<style>";
    let close_tag = "</style>";
    if let (Some(open), Some(close)) = (document.find(open_tag), document.find(close_tag)) {
        if close > open {
            let css_start = open + open_tag.len();
            let css = document[css_start..close].trim().to_string();
            let mut html = String::with_capacity(document.len() - (close + close_tag.len() - open));
            html.push_str(document[..open].trim());
            html.push('\n');
            html.push_str(document[close + close_tag.len()..].trim());
            return (html, css);
        }
    }
    (document.to_string(), String::new())
}

fn merge_css(parts: &[&str]) -> String {
    let mut css = String::new();
    for part in parts {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if !css.is_empty() {
            css.push('\n');
        }
        css.push_str(p);
    }
    css
}

fn replace_first_content_image(
    input: &str,
    path: &str,
    fit: Option<ImageFit>,
    clip_radius: Option<i32>,
) -> String {
    let mut search_from = 0;
    while let Some(rel_start) = input[search_from..].find("<Content") {
        let start = search_from + rel_start;
        let Some(rel_end) = input[start..].find('>') else {
            break;
        };
        let end = start + rel_end + 1;
        let tag = &input[start..end];
        let attrs = extract_content_tag_attrs(tag);
        let tag_type = attrs.get("type").map(|v| v.to_ascii_lowercase());
        if tag_type.as_deref() == Some("image") {
            let mut img = String::from("<img class=\"vk-img\" src=\"");
            img.push_str(&escape_attr_value(path));
            let wants_cover = matches!(fit, Some(ImageFit::Cover))
                || matches!(attrs.get("fit").map(String::as_str), Some("cover"));
            if wants_cover {
                img.push_str("\" data-vk-fit=\"cover");
            } else {
                img.push('"');
            }

            if let Some(radius) = clip_radius.or_else(|| attrs.get("clip-radius").and_then(|v| v.parse::<i32>().ok())) {
                img.push_str("\" data-vk-clip-radius=\"");
                img.push_str(&radius.to_string());
            }
            img.push_str("\" />");

            let mut out = String::with_capacity(input.len() - (end - start) + img.len());
            out.push_str(&input[..start]);
            out.push_str(&img);
            out.push_str(&input[end..]);
            return out;
        }
        search_from = end;
    }
    input.to_string()
}

fn extract_content_tag_attrs(tag: &str) -> std::collections::BTreeMap<String, String> {
    let mut attrs = std::collections::BTreeMap::new();
    let body = tag.strip_prefix('<').and_then(|s| s.strip_suffix('>')).unwrap_or(tag).trim();
    let mut parts = body.split_whitespace();
    let _ = parts.next();
    for part in parts {
        if let Some((key, value)) = part.split_once('=') {
            attrs.insert(
                key.trim().to_ascii_lowercase(),
                value.trim_matches('/').trim_matches('"').trim_matches('\'').to_string(),
            );
        }
    }
    attrs
}

fn replace_first_content_text(input: &str, replacement: &str) -> String {
    for pat in [
        "<Content type=\"String\" />",
        "<Content type=\"String\"/>",
        "<Content type='String' />",
        "<Content type='String'/>",
        "<Content type=\"Text\" />",
        "<Content type=\"Text\"/>",
        "<Content type='Text' />",
        "<Content type='Text'/>",
        "<Content type=\"text\" />",
        "<Content type=\"text\"/>",
        "<Content type='text' />",
        "<Content type='text'/>",
    ] {
        if let Some(pos) = input.find(pat) {
            let mut out = String::with_capacity(input.len() - pat.len() + replacement.len());
            out.push_str(&input[..pos]);
            out.push_str(replacement);
            out.push_str(&input[pos + pat.len()..]);
            return out;
        }
    }
    input.to_string()
}

fn replace_size_placeholders(css: &str, width: Option<&String>, height: Option<&String>) -> String {
    let mut out = css.to_string();
    if let Some(width) = width {
        out = out.replace("CONTENT_W", width);
    }
    if let Some(height) = height {
        out = out.replace("CONTENT_H", height);
    }
    out
}

fn escape_attr_value(s: &str) -> String {
    // Minimal attribute escaping for our generated HTML.
    s.replace('&', "&amp;")
        .replace('\"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
