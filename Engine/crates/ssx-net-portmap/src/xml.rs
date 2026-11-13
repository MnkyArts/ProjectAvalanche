use quick_xml::Reader;
use quick_xml::events::Event;

use crate::Error;

// PORT(choice): 64 KiB (the HTTP body's cap), 16 levels (a description nests 8: root, device, deviceList, device,
// deviceList, device, serviceList, service), 2 048 elements, 4 KiB of text in one; tuning: real gateways' descriptions;
// the original has no dedicated server.
pub const MAX_DOCUMENT: usize = 64 * 1024;
const MAX_DEPTH: usize = 16;
const MAX_ELEMENTS: usize = 2048;
const MAX_TEXT: usize = 4096;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Element {
    pub name: String,
    pub children: Vec<Element>,
    pub text: String,
}

impl Element {
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Element> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    pub fn text_of(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.trim())
    }

    pub fn walk(&self) -> Vec<&Element> {
        let mut out = vec![self];
        let mut i = 0;
        while let Some(e) = out.get(i).copied() {
            out.extend(e.children.iter());
            i += 1;
        }
        out
    }
}

fn bad(what: impl Into<String>) -> Error {
    Error::malformed(format!("XML: {}", what.into()))
}

pub fn parse(doc: &[u8]) -> Result<Element, Error> {
    if doc.len() > MAX_DOCUMENT {
        return Err(bad(format!("larger than {MAX_DOCUMENT} bytes")));
    }
    let text = std::str::from_utf8(doc.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(doc)).map_err(|_| bad("not UTF-8"))?;
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    let mut elements = 0usize;
    loop {
        let event = reader.read_event().map_err(|e| bad(format!("at byte {}: {e}", reader.buffer_position())))?;
        match event {
            Event::Start(_) | Event::Empty(_) if root.is_some() => return Err(bad("a second root element")),
            Event::Start(e) => {
                elements += 1;
                if stack.len() >= MAX_DEPTH || elements > MAX_ELEMENTS {
                    return Err(bad("deeper than 16 levels or more than 2 048 elements"));
                }
                stack.push(Element { name: local(e.local_name().as_ref())?, ..Default::default() });
            }
            Event::Empty(e) => {
                elements += 1;
                if stack.len() >= MAX_DEPTH || elements > MAX_ELEMENTS {
                    return Err(bad("deeper than 16 levels or more than 2 048 elements"));
                }
                let el = Element { name: local(e.local_name().as_ref())?, ..Default::default() };
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None => root = Some(el),
                }
            }
            Event::End(e) => {
                let Some(el) = stack.pop() else { return Err(bad("an end tag without a start")) };
                if local(e.local_name().as_ref())? != el.name {
                    return Err(bad(format!(
                        "</{}> closes <{}>",
                        short(&local(e.local_name().as_ref())?),
                        short(&el.name)
                    )));
                }
                match stack.last_mut() {
                    Some(parent) => parent.children.push(el),
                    None => root = Some(el),
                }
            }
            Event::Text(t) => push_text(&mut stack, &t.xml10_content().map_err(|e| bad(e.to_string()))?)?,
            Event::CData(c) => {
                let s = c.decode().map_err(|e| bad(e.to_string()))?;
                push_text(&mut stack, &s)?;
            }
            Event::GeneralRef(r) => {
                let c = match r.resolve_char_ref().map_err(|e| bad(e.to_string()))? {
                    Some(c) => c,
                    None => match r.decode().map_err(|e| bad(e.to_string()))?.as_ref() {
                        "amp" => '&',
                        "lt" => '<',
                        "gt" => '>',
                        "quot" => '"',
                        "apos" => '\'',
                        other => return Err(bad(format!("the entity &{}; (no entity is expanded)", short(other)))),
                    },
                };
                push_text(&mut stack, c.encode_utf8(&mut [0u8; 4]))?;
            }
            Event::DocType(_) => return Err(bad("a DOCTYPE (no DTD is read)")),
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
            Event::Eof => break,
        }
    }
    if !stack.is_empty() {
        return Err(bad("the document ends inside an element"));
    }
    root.ok_or_else(|| bad("no root element"))
}

fn local(name: &[u8]) -> Result<String, Error> {
    std::str::from_utf8(name).map(str::to_string).map_err(|_| bad("a name that is not UTF-8"))
}

fn short(s: &str) -> String {
    ssx_core::text::shown_max(s, 40).to_string()
}

fn push_text(stack: &mut [Element], s: &str) -> Result<(), Error> {
    let Some(el) = stack.last_mut() else {
        return if s.trim().is_empty() { Ok(()) } else { Err(bad("text outside the root element")) };
    };
    if el.text.len() + s.len() > MAX_TEXT {
        return Err(bad(format!("an element with more than {MAX_TEXT} bytes of text")));
    }
    el.text.push_str(s);
    Ok(())
}

pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_and_hostile_ones() {
        let d = parse(
            b"<?xml version=\"1.0\"?><a:root xmlns:a=\"x\"><b>one &amp; &#50;</b><c/><b><![CDATA[<3>]]></b></a:root>",
        )
        .expect("reads");
        assert_eq!(d.name, "root");
        assert_eq!(d.children_named("b").map(|b| b.text.as_str()).collect::<Vec<_>>(), ["one & 2", "<3>"]);
        assert_eq!(d.walk().len(), 4);
        let laughs = b"<?xml version=\"1.0\"?><!DOCTYPE lolz [<!ENTITY lol \"lol\"><!ENTITY lol2 \"&lol;&lol;&lol;&lol;\">]><lolz>&lol2;</lolz>";
        assert!(parse(laughs).unwrap_err().to_string().contains("DOCTYPE"));
        assert!(parse(b"<a>&lol2;</a>").unwrap_err().to_string().contains("entity"));
        assert!(parse(b"<a>&#0;</a>").is_err());
        assert!(parse("<a>".repeat(17).as_bytes()).is_err());
        let wide = format!("<a>{}</a>", "<b/>".repeat(2048));
        assert!(parse(wide.as_bytes()).unwrap_err().to_string().contains("2 048"));
        assert!(parse(format!("<a>{}</a>", "x".repeat(4097)).as_bytes()).unwrap_err().to_string().contains("text"));
        assert!(parse(&vec![b' '; MAX_DOCUMENT + 1]).unwrap_err().to_string().contains("larger"));
        assert!(parse(b"<a></b>").is_err());
        assert!(parse(b"<a/><b/>").unwrap_err().to_string().contains("second root"));
        assert!(parse(b"<a>").unwrap_err().to_string().contains("ends inside"));
        assert!(parse(b"").is_err());
        assert_eq!(escape("a<&>\"'"), "a&lt;&amp;&gt;&quot;&apos;");
    }
}
