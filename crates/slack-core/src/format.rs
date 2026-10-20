//! Slack message formatting → a small document model the UI can render.
//!
//! Two sources: `rich_text` blocks (precise, sent by the official clients) and legacy
//! mrkdwn `text` (bots, older messages). Both become `Vec<Block>`.

use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub code: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    Text { text: String, style: Style },
    User { id: String },
    Channel { id: String, name: Option<String> },
    /// `@here`, `@channel`, `@everyone`
    Broadcast(String),
    Usergroup { id: String, handle: Option<String> },
    Link { url: String, label: Option<String>, style: Style },
    /// `unicode` is the hex code point sequence from rich_text (`1f44b`, `1f468-200d-1f4bb`).
    Emoji { name: String, unicode: Option<String> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    Quote(Vec<Inline>),
    Code(String),
    List { ordered: bool, indent: u8, items: Vec<Vec<Inline>> },
}

fn text(s: &str, style: Style) -> Inline {
    Inline::Text { text: s.to_string(), style }
}

/// Decode the three HTML entities Slack escapes in text.
pub fn unescape(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

// ---- rich_text blocks ---------------------------------------------------------------

fn style_of(v: &Value) -> Style {
    let s = &v["style"];
    Style {
        bold: s["bold"].as_bool().unwrap_or(false),
        italic: s["italic"].as_bool().unwrap_or(false),
        strike: s["strike"].as_bool().unwrap_or(false),
        code: s["code"].as_bool().unwrap_or(false),
    }
}

fn rich_inlines(elements: &Value) -> Vec<Inline> {
    let mut out = Vec::new();
    for el in elements.as_array().into_iter().flatten() {
        let s = |k: &str| el[k].as_str().map(String::from);
        match el["type"].as_str().unwrap_or("") {
            "text" => out.push(text(el["text"].as_str().unwrap_or(""), style_of(el))),
            // The address comes HTML-escaped (`&amp;`), as in mrkdwn text.
            "link" => out.push(Inline::Link { url: unescape(el["url"].as_str().unwrap_or("")), label: s("text"), style: style_of(el) }),
            "user" => out.push(Inline::User { id: s("user_id").unwrap_or_default() }),
            "channel" => out.push(Inline::Channel { id: s("channel_id").unwrap_or_default(), name: None }),
            "usergroup" => out.push(Inline::Usergroup { id: s("usergroup_id").unwrap_or_default(), handle: None }),
            "broadcast" => out.push(Inline::Broadcast(s("range").unwrap_or_else(|| "here".into()))),
            "emoji" => out.push(Inline::Emoji { name: s("name").unwrap_or_default(), unicode: s("unicode") }),
            "date" => out.push(text(el["fallback"].as_str().unwrap_or(""), Style::default())),
            "color" => out.push(text(el["value"].as_str().unwrap_or(""), Style::default())),
            _ => {}
        }
    }
    out
}

fn preformatted_text(elements: &Value) -> String {
    rich_inlines(elements)
        .into_iter()
        .map(|i| match i {
            Inline::Text { text, .. } => text,
            Inline::Link { url, label, .. } => label.unwrap_or(url),
            Inline::Emoji { name, .. } => format!(":{name}:"),
            Inline::User { id } => format!("<@{id}>"),
            _ => String::new(),
        })
        .collect()
}

/// Parse a message's `rich_text` blocks; `None` if there are none.
pub fn from_rich_text(blocks: &[Value]) -> Option<Vec<Block>> {
    let mut out = Vec::new();
    let mut found = false;
    for b in blocks.iter().filter(|b| b["type"] == "rich_text") {
        found = true;
        for el in b["elements"].as_array().into_iter().flatten() {
            match el["type"].as_str().unwrap_or("") {
                "rich_text_section" => out.push(Block::Paragraph(rich_inlines(&el["elements"]))),
                "rich_text_quote" => out.push(Block::Quote(rich_inlines(&el["elements"]))),
                "rich_text_preformatted" => out.push(Block::Code(preformatted_text(&el["elements"]))),
                "rich_text_list" => {
                    let items = el["elements"].as_array().into_iter().flatten().map(|item| rich_inlines(&item["elements"])).collect();
                    out.push(Block::List {
                        ordered: el["style"] == "ordered",
                        indent: el["indent"].as_u64().unwrap_or(0).min(8) as u8,
                        items,
                    });
                }
                _ => {}
            }
        }
    }
    found.then_some(out)
}

// ---- mrkdwn text --------------------------------------------------------------------

/// Parse `<…>` entities: mentions, channels, broadcasts, links.
fn angle_entity(inner: &str, style: Style) -> Inline {
    let (target, label) = match inner.split_once('|') {
        Some((t, l)) => (t, Some(unescape(l))),
        None => (inner, None),
    };
    if let Some(id) = target.strip_prefix('@') {
        return Inline::User { id: id.to_string() };
    }
    if let Some(id) = target.strip_prefix('#') {
        return Inline::Channel { id: id.to_string(), name: label };
    }
    if let Some(rest) = target.strip_prefix('!') {
        if let Some(id) = rest.strip_prefix("subteam^") {
            return Inline::Usergroup { id: id.to_string(), handle: label };
        }
        if let Some(date) = rest.strip_prefix("date^") {
            // <!date^ts^format|fallback>
            return text(&label.unwrap_or_else(|| date.to_string()), style);
        }
        return Inline::Broadcast(rest.to_string());
    }
    Inline::Link { url: unescape(target), label, style }
}

/// Is `c` a character that may border a formatting marker?
fn boundary(c: Option<char>) -> bool {
    c.is_none_or(|c| c.is_whitespace() || "(){}[]<>.,;:!?'\"-/".contains(c))
}

/// Inline mrkdwn: `*bold*`, `_italic_`, `~strike~`, `` `code` ``, `<…>`, `:emoji:`.
pub fn parse_inline(src: &str) -> Vec<Inline> {
    let chars: Vec<char> = src.chars().collect();
    let mut out: Vec<Inline> = Vec::new();
    let mut buf = String::new();
    let mut style = Style::default();
    let mut i = 0;
    let flush = |buf: &mut String, out: &mut Vec<Inline>, style: Style| {
        if !buf.is_empty() {
            out.push(text(&unescape(buf), style));
            buf.clear();
        }
    };
    while i < chars.len() {
        let c = chars[i];
        let prev = if i == 0 { None } else { Some(chars[i - 1]) };
        match c {
            '`' if !style.code => {
                if let Some(end) = chars[i + 1..].iter().position(|&x| x == '`') {
                    flush(&mut buf, &mut out, style);
                    let code: String = chars[i + 1..i + 1 + end].iter().collect();
                    out.push(text(&unescape(&code), Style { code: true, ..style }));
                    i += end + 2;
                    continue;
                }
                buf.push(c);
            }
            '<' => {
                if let Some(end) = chars[i + 1..].iter().position(|&x| x == '>') {
                    flush(&mut buf, &mut out, style);
                    let inner: String = chars[i + 1..i + 1 + end].iter().collect();
                    out.push(angle_entity(&inner, style));
                    i += end + 2;
                    continue;
                }
                buf.push(c);
            }
            ':' => {
                // :emoji_name: (letters, digits, _ + - ')
                let rest = &chars[i + 1..];
                let end = rest.iter().position(|&x| x == ':');
                if let Some(end) = end.filter(|&e| e > 0 && e <= 64) {
                    let name: String = rest[..end].iter().collect();
                    if name.chars().all(|x| x.is_ascii_alphanumeric() || "_+-'".contains(x)) {
                        flush(&mut buf, &mut out, style);
                        out.push(Inline::Emoji { name, unicode: None });
                        i += end + 2;
                        continue;
                    }
                }
                buf.push(c);
            }
            '*' | '_' | '~' => {
                let next = chars.get(i + 1).copied();
                let flag = match c {
                    '*' => &mut style.bold,
                    '_' => &mut style.italic,
                    _ => &mut style.strike,
                };
                let opening = !*flag && boundary(prev) && next.is_some_and(|n| !n.is_whitespace())
                    && chars[i + 1..].contains(&c);
                let closing = *flag && prev.is_some_and(|p| !p.is_whitespace()) && boundary(next);
                if opening || closing {
                    flush(&mut buf, &mut out, style);
                    let flag = match c {
                        '*' => &mut style.bold,
                        '_' => &mut style.italic,
                        _ => &mut style.strike,
                    };
                    *flag = !*flag;
                } else {
                    buf.push(c);
                }
            }
            _ => buf.push(c),
        }
        i += 1;
    }
    flush(&mut buf, &mut out, style);
    out
}

/// Block-level mrkdwn: ``` fences and `>` quotes; other lines become paragraphs.
pub fn from_mrkdwn(src: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut rest = src;
    while !rest.is_empty() {
        if let Some(start) = rest.find("```") {
            let before = &rest[..start];
            push_lines(before, &mut out);
            let after = &rest[start + 3..];
            match after.find("```") {
                Some(end) => {
                    out.push(Block::Code(unescape(after[..end].trim_matches('\n'))));
                    rest = &after[end + 3..];
                }
                None => {
                    push_lines(&rest[start..], &mut out);
                    rest = "";
                }
            }
        } else {
            push_lines(rest, &mut out);
            rest = "";
        }
    }
    out
}

fn push_lines(s: &str, out: &mut Vec<Block>) {
    // The newline that separates text from a following/preceding ``` fence is not a line.
    let s = s.strip_suffix('\n').unwrap_or(s);
    let s = s.strip_prefix('\n').unwrap_or(s);
    if s.is_empty() {
        return;
    }
    for line in s.split('\n') {
        if let Some(q) = line.strip_prefix("&gt; ").or_else(|| line.strip_prefix("&gt;")).or_else(|| line.strip_prefix("> ")) {
            out.push(Block::Quote(parse_inline(q)));
        } else {
            out.push(Block::Paragraph(parse_inline(line)));
        }
    }
}

/// Best document for a message: rich_text when present, else mrkdwn text.
pub fn message_blocks(text: &str, blocks: &[Value]) -> Vec<Block> {
    from_rich_text(blocks).filter(|b| !b.is_empty()).unwrap_or_else(|| from_mrkdwn(text))
}

/// Decode a rich_text `unicode` value (`1f468-200d-1f4bb`) into a string.
pub fn decode_unicode(hex: &str) -> Option<String> {
    hex.split('-').map(|h| u32::from_str_radix(h, 16).ok().and_then(char::from_u32)).collect()
}

/// Plain-text rendering for notifications and previews, resolving names via `name_of`.
pub fn plain_text(blocks: &[Block], name_of: &dyn Fn(&Inline) -> String) -> String {
    let inline = |items: &[Inline]| items.iter().map(name_of).collect::<String>();
    blocks
        .iter()
        .map(|b| match b {
            Block::Paragraph(i) => inline(i),
            Block::Quote(i) => format!("> {}", inline(i)),
            Block::Code(c) => c.clone(),
            Block::List { items, .. } => items.iter().map(|i| format!("• {}", inline(i))).collect::<Vec<_>>().join("\n"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b() -> Style {
        Style { bold: true, ..Default::default() }
    }

    #[test]
    fn inline_styles() {
        let got = parse_inline("hi *there* _you_ ~old~ `x*y`");
        assert_eq!(
            got,
            vec![
                text("hi ", Style::default()),
                text("there", b()),
                text(" ", Style::default()),
                text("you", Style { italic: true, ..Default::default() }),
                text(" ", Style::default()),
                text("old", Style { strike: true, ..Default::default() }),
                text(" ", Style::default()),
                text("x*y", Style { code: true, ..Default::default() }),
            ]
        );
    }

    #[test]
    fn no_false_markers() {
        assert_eq!(parse_inline("snake_case_name 2*3*4"), vec![text("snake_case_name 2*3*4", Style::default())]);
    }

    #[test]
    fn entities() {
        let got = parse_inline("<@U1> in <#C1|general> <!here> <https://x.y|site> &lt;3 :wave:");
        assert_eq!(got[0], Inline::User { id: "U1".into() });
        assert_eq!(got[2], Inline::Channel { id: "C1".into(), name: Some("general".into()) });
        assert_eq!(got[4], Inline::Broadcast("here".into()));
        assert_eq!(got[6], Inline::Link { url: "https://x.y".into(), label: Some("site".into()), style: Style::default() });
        assert_eq!(got[7], text(" <3 ", Style::default()));
        assert_eq!(got[8], Inline::Emoji { name: "wave".into(), unicode: None });
    }

    #[test]
    fn unicode_emoji() {
        assert_eq!(decode_unicode("1f44b").as_deref(), Some("👋"));
        assert_eq!(decode_unicode("1f468-200d-1f4bb").as_deref(), Some("👨‍💻"));
        assert_eq!(decode_unicode("zz"), None);
    }

    #[test]
    fn blocks_from_mrkdwn() {
        let got = from_mrkdwn("line\n&gt; quoted\n```let x = 1;```");
        assert_eq!(got.len(), 3);
        assert!(matches!(&got[1], Block::Quote(_)));
        assert_eq!(got[2], Block::Code("let x = 1;".into()));
    }

    #[test]
    fn rich_text_parsing() {
        let blocks: Vec<Value> = serde_json::from_str(
            r#"[{"type":"rich_text","elements":[
                {"type":"rich_text_section","elements":[{"type":"text","text":"bold","style":{"bold":true}},{"type":"user","user_id":"U2"}]},
                {"type":"rich_text_list","style":"ordered","indent":0,"elements":[{"type":"rich_text_section","elements":[{"type":"text","text":"one"}]}]},
                {"type":"rich_text_preformatted","elements":[{"type":"text","text":"code"}]}
            ]}]"#,
        )
        .unwrap();
        let doc = from_rich_text(&blocks).unwrap();
        assert_eq!(doc[0], Block::Paragraph(vec![text("bold", b()), Inline::User { id: "U2".into() }]));
        assert!(matches!(&doc[1], Block::List { ordered: true, .. }));
        assert_eq!(doc[2], Block::Code("code".into()));
        assert!(from_rich_text(&[]).is_none());
    }
}
