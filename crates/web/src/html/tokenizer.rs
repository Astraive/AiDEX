//! A small, forgiving HTML tokenizer.
//!
//! It yields start tags (lowercase names, entity-decoded attributes), end
//! tags, and raw text; comments, doctypes, and processing instructions are
//! skipped. The content of raw-text elements (`script`, `style`, `title`,
//! `textarea`, ...) is returned as one text token up to the matching end tag,
//! so markup inside a script never becomes tags. Malformed markup degrades
//! to text instead of failing.

use super::entities;

/// Elements whose content is raw text up to their end tag.
const RAW_TEXT: &[&str] = &[
    "script", "style", "title", "textarea", "xmp", "iframe", "noembed", "noframes",
];
/// Most attributes kept per tag.
const MAX_ATTRIBUTES: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Token<'a> {
    Start {
        name: String,
        attributes: Vec<(String, String)>,
        self_closing: bool,
    },
    End {
        name: String,
    },
    /// Undecoded text (decode with `entities::decode`; raw-text element
    /// content is passed through the same way).
    Text(&'a str),
}

impl Token<'_> {
    pub fn attribute(&self, wanted: &str) -> Option<&str> {
        match self {
            Self::Start { attributes, .. } => attributes
                .iter()
                .find(|(name, _)| name == wanted)
                .map(|(_, value)| value.as_str()),
            _ => None,
        }
    }
}

pub struct Tokenizer<'a> {
    input: &'a str,
    position: usize,
    /// Inside a raw-text element: its name.
    raw: Option<String>,
}

impl<'a> Tokenizer<'a> {
    pub fn new(input: &'a str) -> Self {
        Self {
            input,
            position: 0,
            raw: None,
        }
    }

    fn rest(&self) -> &'a str {
        &self.input[self.position..]
    }

    /// Text of a raw-text element, up to (not including) `</name`.
    fn raw_text(&mut self, name: &str) -> &'a str {
        let rest = self.rest();
        let bytes = rest.as_bytes();
        let mut index = 0;
        while let Some(found) = rest[index..].find("</") {
            let at = index + found;
            let candidate = &bytes[at + 2..];
            let matches = candidate.len() >= name.len()
                && candidate[..name.len()].eq_ignore_ascii_case(name.as_bytes())
                && candidate
                    .get(name.len())
                    .is_none_or(|next| next.is_ascii_whitespace() || matches!(next, b'>' | b'/'));
            if matches {
                self.position += at;
                return &rest[..at];
            }
            index = at + 2;
        }
        self.position = self.input.len();
        rest
    }

    /// Skip to just after the next `>` (or the end).
    fn skip_past(&mut self, needle: &str) {
        match self.rest().find(needle) {
            Some(found) => self.position += found + needle.len(),
            None => self.position = self.input.len(),
        }
    }

    fn tag(&mut self) -> Option<Token<'a>> {
        let rest = self.rest();
        let bytes = rest.as_bytes();
        let closing = bytes.get(1) == Some(&b'/');
        let name_start = if closing { 2 } else { 1 };
        let name_length = bytes[name_start..]
            .iter()
            .take_while(|byte| !byte.is_ascii_whitespace() && !matches!(byte, b'/' | b'>'))
            .count();
        let name = rest[name_start..name_start + name_length].to_ascii_lowercase();
        let mut index = name_start + name_length;
        let mut attributes = Vec::new();
        let mut self_closing = false;
        loop {
            while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
                index += 1;
            }
            match bytes.get(index) {
                None => {
                    self.position = self.input.len();
                    break;
                }
                Some(b'>') => {
                    self.position += index + 1;
                    break;
                }
                Some(b'/') => {
                    self_closing = bytes.get(index + 1) == Some(&b'>');
                    index += 1;
                    continue;
                }
                Some(_) => {}
            }
            let attribute_start = index;
            while bytes.get(index).is_some_and(|byte| {
                !byte.is_ascii_whitespace() && !matches!(byte, b'=' | b'>' | b'/')
            }) {
                index += 1;
            }
            let attribute = rest[attribute_start..index].to_ascii_lowercase();
            while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
                index += 1;
            }
            let mut value = String::new();
            if bytes.get(index) == Some(&b'=') {
                index += 1;
                while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
                    index += 1;
                }
                match bytes.get(index) {
                    Some(&quote @ (b'"' | b'\'')) => {
                        let start = index + 1;
                        let end = rest[start..]
                            .find(quote as char)
                            .map_or(rest.len(), |found| start + found);
                        value = entities::decode(&rest[start..end]).into_owned();
                        index = (end + 1).min(rest.len());
                    }
                    _ => {
                        let start = index;
                        while bytes
                            .get(index)
                            .is_some_and(|byte| !byte.is_ascii_whitespace() && *byte != b'>')
                        {
                            index += 1;
                        }
                        value = entities::decode(&rest[start..index]).into_owned();
                    }
                }
            }
            if !attribute.is_empty() && attributes.len() < MAX_ATTRIBUTES {
                attributes.push((attribute, value));
            }
        }
        if closing {
            return Some(Token::End { name });
        }
        if RAW_TEXT.contains(&name.as_str()) && !self_closing {
            self.raw = Some(name.clone());
        }
        Some(Token::Start {
            name,
            attributes,
            self_closing,
        })
    }
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(name) = self.raw.take() {
                let text = self.raw_text(&name);
                if !text.is_empty() {
                    return Some(Token::Text(text));
                }
                continue;
            }
            let rest = self.rest();
            if rest.is_empty() {
                return None;
            }
            if !rest.starts_with('<') {
                let length = rest.find('<').unwrap_or(rest.len());
                self.position += length;
                return Some(Token::Text(&rest[..length]));
            }
            let next = rest.as_bytes().get(1).copied();
            match next {
                Some(b'!') if rest.starts_with("<!--") => {
                    self.position += 4;
                    self.skip_past("-->");
                }
                // Bytes, not a `str` slice: byte 9 may fall inside a multi-byte character.
                // A match means bytes 0..9 are ASCII, so `rest[9..]` is on a boundary.
                Some(b'!')
                    if rest
                        .as_bytes()
                        .get(..9)
                        .is_some_and(|head| head.eq_ignore_ascii_case(b"<![CDATA[")) =>
                {
                    let body = &rest[9..];
                    let length = body.find("]]>").unwrap_or(body.len());
                    self.position += 9 + length;
                    self.skip_past("]]>");
                    if length > 0 {
                        return Some(Token::Text(&body[..length]));
                    }
                }
                Some(b'!' | b'?') => self.skip_past(">"),
                Some(b'/') if rest.as_bytes().get(2).is_some_and(u8::is_ascii_alphabetic) => {
                    return self.tag();
                }
                Some(byte) if byte.is_ascii_alphabetic() => return self.tag(),
                _ => {
                    // A lone `<` is text.
                    self.position += 1;
                    return Some(Token::Text(&rest[..1]));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(html: &str) -> Vec<Token<'_>> {
        Tokenizer::new(html).collect()
    }

    #[test]
    fn tags_attributes_and_text_are_tokenized() {
        let found = tokens(
            "<!DOCTYPE html><!-- note --><A HREF='/x?a=1&amp;b=2' class=link data-x>Hi &amp; bye</a><br/>",
        );
        assert_eq!(
            found,
            vec![
                Token::Start {
                    name: "a".into(),
                    attributes: vec![
                        ("href".into(), "/x?a=1&b=2".into()),
                        ("class".into(), "link".into()),
                        ("data-x".into(), String::new()),
                    ],
                    self_closing: false,
                },
                Token::Text("Hi &amp; bye"),
                Token::End { name: "a".into() },
                Token::Start {
                    name: "br".into(),
                    attributes: vec![],
                    self_closing: true,
                },
            ]
        );
    }

    #[test]
    fn raw_text_elements_hide_markup_until_their_end_tag() {
        let found = tokens("<script>if (a < b) { x = '</div>'; }</SCRIPT ><p>after</p>");
        assert_eq!(found[1], Token::Text("if (a < b) { x = '</div>'; }"));
        assert_eq!(
            found[2],
            Token::End {
                name: "script".into()
            }
        );
        assert_eq!(found[4], Token::Text("after"));
        // An unterminated script swallows the rest instead of leaking it.
        let found = tokens("<style>body{}<p>never");
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn malformed_markup_degrades_to_text() {
        let found = tokens("1 < 2 and <3 <![CDATA[x<y]]> <? pi ?>end <div");
        let text: String = found
            .iter()
            .filter_map(|token| match token {
                Token::Text(text) => Some(*text),
                _ => None,
            })
            .collect();
        assert_eq!(text, "1 < 2 and <3 x<y end ");
        assert!(matches!(found.last(), Some(Token::Start { name, .. }) if name == "div"));
    }

    #[test]
    fn declarations_followed_by_multi_byte_text_do_not_panic() {
        assert_eq!(tokens("<!中文注释>x"), vec![Token::Text("x")]);
        assert_eq!(tokens("a <!––– b"), vec![Token::Text("a ")]);
        assert!(tokens("<!ééééé").is_empty());
        assert_eq!(tokens("<![cdata[é]]>"), vec![Token::Text("é")]);
        assert_eq!(tokens("<![CDATA[中"), vec![Token::Text("中")]);
        assert_eq!(tokens("<!-中文>y"), vec![Token::Text("y")]);
        assert_eq!(tokens("<?中文注释?>z"), vec![Token::Text("z")]);
        // A multi-byte character at every byte offset after `<!`.
        for pad in 0..12 {
            let html = format!("<!{}\u{1f600}\u{e9}>x", "-".repeat(pad));
            let found = tokens(&html);
            assert!(found.len() <= 1, "{html}: {found:?}");
        }
    }

    #[test]
    fn markup_cut_off_at_the_end_does_not_panic() {
        for html in [
            "<",
            "<!",
            "<!-",
            "<!--",
            "<!--x",
            "<![CDATA",
            "<![CDATA[",
            "<?",
        ] {
            let found = tokens(html);
            assert!(
                found.iter().all(|token| matches!(token, Token::Text("<"))),
                "{html}: {found:?}"
            );
        }
        assert_eq!(tokens("x<!-"), vec![Token::Text("x")]);
        assert_eq!(tokens("</"), vec![Token::Text("<"), Token::Text("/")]);
        assert_eq!(tokens("</é>"), vec![Token::Text("<"), Token::Text("/é>")]);
        assert_eq!(tokens("</ a>"), vec![Token::Text("<"), Token::Text("/ a>")]);
        assert_eq!(tokens("</a"), vec![Token::End { name: "a".into() }]);
        assert_eq!(tokens("</aé"), vec![Token::End { name: "aé".into() }]);
        assert_eq!(tokens("</p é=\"x"), vec![Token::End { name: "p".into() }]);
        assert_eq!(
            tokens("<a/"),
            vec![Token::Start {
                name: "a".into(),
                attributes: vec![],
                self_closing: false,
            }]
        );
        // Raw text that ends inside a candidate end tag.
        assert_eq!(
            tokens("<script>中</scr"),
            vec![
                Token::Start {
                    name: "script".into(),
                    attributes: vec![],
                    self_closing: false,
                },
                Token::Text("中</scr"),
            ]
        );
    }
}
