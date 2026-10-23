//! HTML DOM, CSS styling, and SVG vector parsing for the WebKit page engine.

use alloc::string::String;
use alloc::vec::Vec;

/// Color representation (R, G, B).
pub type Color = (u8, u8, u8);

/// Parsed CSS style declarations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CssStyle {
    pub background: Option<Color>,
    pub color: Option<Color>,
    pub font_size: Option<u32>,
    pub margin_top: i32,
    pub margin_bottom: i32,
    pub margin_left: i32,
    pub margin_right: i32,
    pub padding: i32,
    pub border_top_width: i32,
    pub border_top_color: Option<Color>,
    pub width: Option<i32>,
    pub height: Option<i32>,
}

/// Single CSS rule with selector and styling properties.
#[derive(Debug, Clone)]
pub struct CssRule {
    pub selector: String,
    pub style: CssStyle,
}

/// SVG command in `<path d="...">`.
#[derive(Debug, Clone, PartialEq)]
pub enum SvgCommand {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    HorizontalTo(f32),
    VerticalTo(f32),
    Close,
}

/// SVG vector graphic parsed from SVG markup.
#[derive(Debug, Clone, Default)]
pub struct SvgGraphic {
    pub width: f32,
    pub height: f32,
    pub rects: Vec<SvgRect>,
    pub paths: Vec<SvgPath>,
}

#[derive(Debug, Clone)]
pub struct SvgRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub rx: f32,
    pub fill: Option<Color>,
}

#[derive(Debug, Clone)]
pub struct SvgPath {
    pub commands: Vec<SvgCommand>,
    pub stroke: Option<Color>,
    pub stroke_width: f32,
    pub fill: Option<Color>,
}

/// A node in the parsed HTML document.
#[derive(Debug, Clone)]
pub enum DomNode {
    Element(DomElement),
    Text(String),
}

/// A DOM element.
#[derive(Debug, Clone)]
pub struct DomElement {
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attributes: Vec<(String, String)>,
    pub children: Vec<DomNode>,
    pub text_content: String,
    pub value: String,
    pub style: CssStyle,
    pub bounds: (i32, i32, i32, i32),
}

impl DomElement {
    pub fn new(tag: &str) -> Self {
        Self {
            tag: String::from(tag).to_ascii_lowercase(),
            id: None,
            classes: Vec::new(),
            attributes: Vec::new(),
            children: Vec::new(),
            text_content: String::new(),
            value: String::new(),
            style: CssStyle::default(),
            bounds: (0, 0, 0, 0),
        }
    }

    pub fn get_attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn set_attribute(&mut self, name: &str, value: &str) {
        if let Some((_, v)) = self
            .attributes
            .iter_mut()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
        {
            *v = String::from(value);
        } else {
            self.attributes.push((String::from(name), String::from(value)));
        }
    }
}

/// Complete parsed HTML document.
#[derive(Debug, Clone, Default)]
pub struct HtmlDocument {
    pub title: String,
    pub body_style: CssStyle,
    pub root_children: Vec<DomNode>,
    pub css_rules: Vec<CssRule>,
    pub inline_scripts: Vec<String>,
    pub dataset: Vec<(String, String)>,
}

impl HtmlDocument {
    pub fn parse(html: &str) -> Self {
        let mut doc = HtmlDocument::default();
        doc.parse_html_internal(html);
        doc
    }

    pub fn find_element_by_id(&self, id: &str) -> Option<&DomElement> {
        fn find<'a>(nodes: &'a [DomNode], id: &str) -> Option<&'a DomElement> {
            for node in nodes {
                if let DomNode::Element(el) = node {
                    if el.id.as_deref() == Some(id) {
                        return Some(el);
                    }
                    if let Some(found) = find(&el.children, id) {
                        return Some(found);
                    }
                }
            }
            None
        }
        find(&self.root_children, id)
    }

    pub fn find_element_by_id_mut(&mut self, id: &str) -> Option<&mut DomElement> {
        fn find<'a>(nodes: &'a mut [DomNode], id: &str) -> Option<&'a mut DomElement> {
            for node in nodes {
                if let DomNode::Element(el) = node {
                    if el.id.as_deref() == Some(id) {
                        return Some(el);
                    }
                    if let Some(found) = find(&mut el.children, id) {
                        return Some(found);
                    }
                }
            }
            None
        }
        find(&mut self.root_children, id)
    }

    pub fn dataset_attr(&self, key: &str) -> Option<&str> {
        self.dataset
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn set_dataset_attr(&mut self, key: &str, value: &str) {
        if let Some((_, v)) = self.dataset.iter_mut().find(|(k, _)| k == key) {
            *v = String::from(value);
        } else {
            self.dataset.push((String::from(key), String::from(value)));
        }
    }

    fn parse_html_internal(&mut self, source: &str) {
        let mut chars = source.chars().peekable();
        let mut node_stack: Vec<DomElement> = Vec::new();

        while let Some(&ch) = chars.peek() {
            if ch == '<' {
                chars.next();
                if chars.peek() == Some(&'!') {
                    // Comment or <!doctype>
                    chars.next();
                    if chars.peek() == Some(&'-') {
                        // Skip <!-- comment -->
                        chars.next();
                        let mut prev2 = ' ';
                        let mut prev1 = ' ';
                        while let Some(c) = chars.next() {
                            if prev2 == '-' && prev1 == '-' && c == '>' {
                                break;
                            }
                            prev2 = prev1;
                            prev1 = c;
                        }
                    } else {
                        // Skip until '>'
                        for c in chars.by_ref() {
                            if c == '>' {
                                break;
                            }
                        }
                    }
                    continue;
                }

                if chars.peek() == Some(&'/') {
                    // Closing tag
                    chars.next();
                    let mut tag_name = String::new();
                    while let Some(&c) = chars.peek() {
                        if c == '>' {
                            chars.next();
                            break;
                        }
                        tag_name.push(c);
                        chars.next();
                    }
                    let tag_name = tag_name.trim().to_ascii_lowercase();
                    if let Some(top) = node_stack.pop() {
                        if top.tag == tag_name || node_stack.is_empty() {
                            self.attach_or_store(top, &mut node_stack);
                        } else {
                            // Mismatched tag, unwind
                            self.attach_or_store(top, &mut node_stack);
                        }
                    }
                    continue;
                }

                // Opening tag
                let mut tag_buf = String::new();
                while let Some(&c) = chars.peek() {
                    if c == '>' {
                        chars.next();
                        break;
                    }
                    tag_buf.push(c);
                    chars.next();
                }

                let is_self_closing = tag_buf.ends_with('/');
                let tag_raw = if is_self_closing {
                    &tag_buf[..tag_buf.len() - 1]
                } else {
                    &tag_buf
                };

                let (tag_name, attrs) = parse_tag_and_attributes(tag_raw);
                let tag_lower = tag_name.to_ascii_lowercase();

                if tag_lower == "title" {
                    let mut title_buf = String::new();
                    while let Some(c) = chars.next() {
                        if c == '<' && chars.peek() == Some(&'/') {
                            // Skip </title>
                            for end_c in chars.by_ref() {
                                if end_c == '>' {
                                    break;
                                }
                            }
                            break;
                        }
                        title_buf.push(c);
                    }
                    self.title = decode_entities(title_buf.trim());
                    continue;
                }

                if tag_lower == "style" {
                    let mut style_buf = String::new();
                    while let Some(c) = chars.next() {
                        if c == '<' && chars.peek() == Some(&'/') {
                            for end_c in chars.by_ref() {
                                if end_c == '>' {
                                    break;
                                }
                            }
                            break;
                        }
                        style_buf.push(c);
                    }
                    self.parse_css(&style_buf);
                    continue;
                }

                if tag_lower == "script" {
                    let mut script_buf = String::new();
                    while let Some(c) = chars.next() {
                        if c == '<' && chars.peek() == Some(&'/') {
                            for end_c in chars.by_ref() {
                                if end_c == '>' {
                                    break;
                                }
                            }
                            break;
                        }
                        script_buf.push(c);
                    }
                    let trimmed = script_buf.trim();
                    if !trimmed.is_empty() {
                        self.inline_scripts.push(String::from(trimmed));
                    }
                    continue;
                }

                let mut element = DomElement::new(&tag_lower);
                for (k, v) in attrs {
                    if k.eq_ignore_ascii_case("id") {
                        element.id = Some(v.clone());
                    } else if k.eq_ignore_ascii_case("class") {
                        element.classes = v.split_whitespace().map(String::from).collect();
                    } else if k.eq_ignore_ascii_case("value") {
                        element.value = v.clone();
                    }
                    element.attributes.push((k, v));
                }

                let is_void_tag = matches!(
                    tag_lower.as_str(),
                    "meta" | "link" | "img" | "input" | "br" | "hr"
                );

                if is_self_closing || is_void_tag {
                    self.attach_or_store(element, &mut node_stack);
                } else {
                    node_stack.push(element);
                }
            } else {
                // Text node
                let mut text_buf = String::new();
                while let Some(&c) = chars.peek() {
                    if c == '<' {
                        break;
                    }
                    text_buf.push(c);
                    chars.next();
                }
                let decoded = decode_entities(&text_buf);
                let trimmed = decoded.trim();
                if !trimmed.is_empty() {
                    if let Some(top) = node_stack.last_mut() {
                        if top.text_content.is_empty() {
                            top.text_content = String::from(trimmed);
                        } else {
                            top.text_content.push(' ');
                            top.text_content.push_str(trimmed);
                        }
                        top.children.push(DomNode::Text(String::from(trimmed)));
                    } else {
                        self.root_children
                            .push(DomNode::Text(String::from(trimmed)));
                    }
                }
            }
        }

        while let Some(top) = node_stack.pop() {
            self.attach_or_store(top, &mut node_stack);
        }

        self.apply_styles();
    }

    fn attach_or_store(&mut self, element: DomElement, stack: &mut [DomElement]) {
        if let Some(parent) = stack.last_mut() {
            parent.children.push(DomNode::Element(element));
        } else {
            self.root_children.push(DomNode::Element(element));
        }
    }

    fn parse_css(&mut self, css: &str) {
        let mut clean = css;
        while let Some(open) = clean.find('{') {
            let selector = clean[..open].trim();
            clean = &clean[open + 1..];
            if let Some(close) = clean.find('}') {
                let decls = &clean[..close];
                clean = &clean[close + 1..];
                let style = parse_css_declarations(decls);
                for sel in selector.split(',') {
                    let s = sel.trim();
                    if !s.is_empty() {
                        self.css_rules.push(CssRule {
                            selector: String::from(s),
                            style: style.clone(),
                        });
                    }
                }
            } else {
                break;
            }
        }
    }

    fn apply_styles(&mut self) {
        for rule in &self.css_rules {
            if rule.selector == "body" {
                if let Some(bg) = rule.style.background {
                    self.body_style.background = Some(bg);
                }
                if let Some(col) = rule.style.color {
                    self.body_style.color = Some(col);
                }
                if rule.style.margin_top > 0 {
                    self.body_style.margin_top = rule.style.margin_top;
                    self.body_style.margin_bottom = rule.style.margin_top;
                    self.body_style.margin_left = rule.style.margin_top;
                    self.body_style.margin_right = rule.style.margin_top;
                }
            }
        }

        fn apply_recursive(node: &mut DomNode, rules: &[CssRule]) {
            if let DomNode::Element(el) = node {
                for rule in rules {
                    let matches_tag = rule.selector == el.tag;
                    let matches_class = rule
                        .selector
                        .strip_prefix('.')
                        .is_some_and(|c| el.classes.iter().any(|ec| ec == c));
                    let matches_id = rule
                        .selector
                        .strip_prefix('#')
                        .is_some_and(|id| el.id.as_deref() == Some(id));

                    if matches_tag || matches_class || matches_id {
                        if rule.style.background.is_some() {
                            el.style.background = rule.style.background;
                        }
                        if rule.style.color.is_some() {
                            el.style.color = rule.style.color;
                        }
                        if rule.style.margin_top != 0 {
                            el.style.margin_top = rule.style.margin_top;
                        }
                        if rule.style.padding != 0 {
                            el.style.padding = rule.style.padding;
                        }
                        if rule.style.border_top_width != 0 {
                            el.style.border_top_width = rule.style.border_top_width;
                            el.style.border_top_color = rule.style.border_top_color;
                        }
                    }
                }

                for child in &mut el.children {
                    apply_recursive(child, rules);
                }
            }
        }

        for child in &mut self.root_children {
            apply_recursive(child, &self.css_rules);
        }
    }
}

fn parse_tag_and_attributes(input: &str) -> (String, Vec<(String, String)>) {
    let mut parts = input.split_whitespace();
    let tag = parts.next().unwrap_or("div");
    let mut attrs = Vec::new();

    let rest = input[tag.len()..].trim();
    let mut chars = rest.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c.is_alphabetic() || c == '_' || c == '-' {
            let mut key = String::new();
            while let Some(&kc) = chars.peek() {
                if kc == '=' || kc.is_whitespace() || kc == '/' || kc == '>' {
                    break;
                }
                key.push(kc);
                chars.next();
            }

            while let Some(&wc) = chars.peek() {
                if wc.is_whitespace() {
                    chars.next();
                } else {
                    break;
                }
            }

            let mut val = String::new();
            if chars.peek() == Some(&'=') {
                chars.next(); // consume '='
                while let Some(&wc) = chars.peek() {
                    if wc.is_whitespace() {
                        chars.next();
                    } else {
                        break;
                    }
                }

                if let Some(&quote) = chars.peek() {
                    if quote == '"' || quote == '\'' {
                        chars.next(); // consume opening quote
                        while let Some(vc) = chars.next() {
                            if vc == quote {
                                break;
                            }
                            val.push(vc);
                        }
                    } else {
                        while let Some(&vc) = chars.peek() {
                            if vc.is_whitespace() || vc == '>' || vc == '/' {
                                break;
                            }
                            val.push(vc);
                            chars.next();
                        }
                    }
                }
            }

            attrs.push((key, decode_entities(&val)));
        } else {
            chars.next();
        }
    }

    (String::from(tag), attrs)
}

fn parse_css_declarations(decls: &str) -> CssStyle {
    let mut style = CssStyle::default();
    for decl in decls.split(';') {
        let mut parts = decl.splitn(2, ':');
        if let (Some(prop), Some(val)) = (parts.next(), parts.next()) {
            let prop = prop.trim().to_ascii_lowercase();
            let val = val.trim();

            match prop.as_str() {
                "background" | "background-color" => {
                    style.background = parse_color(val);
                }
                "color" => {
                    style.color = parse_color(val);
                }
                "padding" => {
                    style.padding = parse_pixels(val);
                }
                "margin" => {
                    let px = parse_pixels(val);
                    style.margin_top = px;
                    style.margin_bottom = px;
                    style.margin_left = px;
                    style.margin_right = px;
                }
                "margin-top" => {
                    style.margin_top = parse_dimension(val);
                }
                "border-top" | "border" => {
                    // e.g. "2px solid #89cfff"
                    for part in val.split_whitespace() {
                        if part.ends_with("px") {
                            style.border_top_width = parse_pixels(part);
                        } else if let Some(c) = parse_color(part) {
                            style.border_top_color = Some(c);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    style
}

pub fn parse_pixels(val: &str) -> i32 {
    let digits: String = val.chars().filter(|c| c.is_ascii_digit()).collect();
    digits.parse::<i32>().unwrap_or(0)
}

pub fn parse_dimension(val: &str) -> i32 {
    if val.ends_with("vh") {
        let vh = val.strip_suffix("vh").unwrap().trim().parse::<i32>().unwrap_or(0);
        (vh * 380) / 100 // Scale to 120vh relative to baseline height
    } else {
        parse_pixels(val)
    }
}

pub fn parse_color(s: &str) -> Option<Color> {
    let s = s.trim().to_ascii_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 3 {
            let r = u8::from_str_radix(&hex[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3].repeat(2), 16).ok()?;
            return Some((r, g, b));
        } else if hex.len() >= 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some((r, g, b));
        }
    }

    match s.as_str() {
        "white" | "#fff" | "#ffffff" => Some((255, 255, 255)),
        "black" | "#000" | "#000000" => Some((0, 0, 0)),
        "red" => Some((255, 0, 0)),
        "green" => Some((0, 255, 0)),
        "blue" => Some((0, 0, 255)),
        _ => None,
    }
}

pub fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

/// Parses SVG XML into an `SvgGraphic` vector description.
pub fn parse_svg(svg_content: &str) -> SvgGraphic {
    let mut graphic = SvgGraphic::default();
    let mut chars = svg_content.chars().peekable();

    while let Some(&c) = chars.peek() {
        if c == '<' {
            chars.next();
            let mut tag_buf = String::new();
            while let Some(&tc) = chars.peek() {
                if tc == '>' {
                    chars.next();
                    break;
                }
                tag_buf.push(tc);
                chars.next();
            }

            let (tag_name, attrs) = parse_tag_and_attributes(&tag_buf);
            let tag_lower = tag_name.to_ascii_lowercase();

            if tag_lower == "svg" {
                for (k, v) in &attrs {
                    if k.eq_ignore_ascii_case("width") {
                        graphic.width = v.parse::<f32>().unwrap_or(64.0);
                    } else if k.eq_ignore_ascii_case("height") {
                        graphic.height = v.parse::<f32>().unwrap_or(64.0);
                    }
                }
            } else if tag_lower == "rect" {
                let mut x = 0.0f32;
                let mut y = 0.0f32;
                let mut width = 0.0f32;
                let mut height = 0.0f32;
                let mut rx = 0.0f32;
                let mut fill = None;

                for (k, v) in &attrs {
                    match k.to_ascii_lowercase().as_str() {
                        "x" => x = v.parse().unwrap_or(0.0),
                        "y" => y = v.parse().unwrap_or(0.0),
                        "width" => width = v.parse().unwrap_or(0.0),
                        "height" => height = v.parse().unwrap_or(0.0),
                        "rx" => rx = v.parse().unwrap_or(0.0),
                        "fill" => fill = parse_color(v),
                        _ => {}
                    }
                }
                graphic.rects.push(SvgRect {
                    x,
                    y,
                    width,
                    height,
                    rx,
                    fill,
                });
            } else if tag_lower == "path" {
                let mut d = String::new();
                let mut stroke = None;
                let mut stroke_width = 1.0f32;
                let mut fill = None;

                for (k, v) in &attrs {
                    match k.to_ascii_lowercase().as_str() {
                        "d" => d = v.clone(),
                        "stroke" => stroke = parse_color(v),
                        "stroke-width" => stroke_width = v.parse().unwrap_or(1.0),
                        "fill" => fill = parse_color(v),
                        _ => {}
                    }
                }

                let commands = parse_path_d(&d);
                graphic.paths.push(SvgPath {
                    commands,
                    stroke,
                    stroke_width,
                    fill,
                });
            }
        } else {
            chars.next();
        }
    }

    if graphic.width == 0.0 {
        graphic.width = 64.0;
    }
    if graphic.height == 0.0 {
        graphic.height = 64.0;
    }

    graphic
}

fn parse_path_d(d: &str) -> Vec<SvgCommand> {
    let mut commands = Vec::new();
    let mut chars = d.chars().peekable();

    let mut current_x = 0.0f32;
    let mut current_y = 0.0f32;

    while let Some(&c) = chars.peek() {
        if c.is_alphabetic() {
            let cmd = chars.next().unwrap();
            match cmd {
                'M' | 'm' => {
                    let nums = read_numbers(&mut chars, 2);
                    if nums.len() >= 2 {
                        if cmd == 'M' {
                            current_x = nums[0];
                            current_y = nums[1];
                        } else {
                            current_x += nums[0];
                            current_y += nums[1];
                        }
                        commands.push(SvgCommand::MoveTo(current_x, current_y));
                    }
                    // Implicit LineTo for subsequent pairs
                    while let Some(&peek) = chars.peek() {
                        if peek.is_alphabetic() {
                            break;
                        }
                        let next_pair = read_numbers(&mut chars, 2);
                        if next_pair.len() >= 2 {
                            if cmd == 'M' {
                                current_x = next_pair[0];
                                current_y = next_pair[1];
                            } else {
                                current_x += next_pair[0];
                                current_y += next_pair[1];
                            }
                            commands.push(SvgCommand::LineTo(current_x, current_y));
                        } else {
                            break;
                        }
                    }
                }
                'L' | 'l' => {
                    let nums = read_numbers(&mut chars, 2);
                    if nums.len() >= 2 {
                        if cmd == 'L' {
                            current_x = nums[0];
                            current_y = nums[1];
                        } else {
                            current_x += nums[0];
                            current_y += nums[1];
                        }
                        commands.push(SvgCommand::LineTo(current_x, current_y));
                    }
                }
                'H' | 'h' => {
                    let nums = read_numbers(&mut chars, 1);
                    if let Some(&x) = nums.first() {
                        if cmd == 'H' {
                            current_x = x;
                        } else {
                            current_x += x;
                        }
                        commands.push(SvgCommand::HorizontalTo(current_x));
                    }
                }
                'V' | 'v' => {
                    let nums = read_numbers(&mut chars, 1);
                    if let Some(&y) = nums.first() {
                        if cmd == 'V' {
                            current_y = y;
                        } else {
                            current_y += y;
                        }
                        commands.push(SvgCommand::VerticalTo(current_y));
                    }
                }
                'Z' | 'z' => {
                    commands.push(SvgCommand::Close);
                }
                _ => {}
            }
        } else {
            chars.next();
        }
    }

    commands
}

fn read_numbers(chars: &mut core::iter::Peekable<core::str::Chars>, count: usize) -> Vec<f32> {
    let mut numbers = Vec::new();
    while numbers.len() < count {
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() || c == ',' {
                chars.next();
            } else {
                break;
            }
        }

        let mut num_str = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_ascii_digit() || c == '.' || c == '-' {
                num_str.push(c);
                chars.next();
            } else {
                break;
            }
        }

        if let Ok(n) = num_str.parse::<f32>() {
            numbers.push(n);
        } else {
            break;
        }
    }
    numbers
}
