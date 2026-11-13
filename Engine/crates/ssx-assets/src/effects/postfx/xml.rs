use anyhow::{Context, Result, bail};

pub(crate) struct Tag<'a> {
    pub(crate) name: &'a str,
    pub(crate) closing: bool,
    pub(crate) self_closing: bool,
    attrs: Vec<(&'a str, &'a str)>,
}

impl Tag<'_> {
    pub(crate) fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }

    fn req(&self, key: &str) -> Result<&str> {
        self.attr(key).with_context(|| format!("<{}> without {key}=", self.name))
    }
}

fn parse_tag(body: &str) -> Result<Tag<'_>> {
    let (closing, body) = match body.strip_prefix('/') {
        Some(rest) => (true, rest),
        None => (false, body),
    };
    let (self_closing, body) = match body.strip_suffix('/') {
        Some(rest) => (true, rest),
        None => (false, body),
    };
    let body = body.trim();
    let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
    let (name, mut rest) = body.split_at(name_end);
    let mut attrs = Vec::new();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        let eq = rest.find('=').with_context(|| format!("attribute without '=' in <{name} {rest}>"))?;
        let key = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'').context("unquoted attribute")?;
        let end = after[1..].find(quote).context("unterminated attribute value")?;
        attrs.push((key, &after[1..1 + end]));
        rest = &after[end + 2..];
    }
    Ok(Tag { name, closing, self_closing, attrs })
}

pub(crate) fn tags(text: &str) -> Result<Vec<Tag<'_>>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        if !rest[..start].trim().is_empty() {
            bail!("unexpected text content {:?}", rest[..start].trim());
        }
        rest = &rest[start..];
        if let Some(after) = rest.strip_prefix("<!--") {
            let end = after.find("-->").context("unterminated comment")?;
            rest = &after[end + 3..];
            continue;
        }
        let end = rest.find('>').context("unterminated tag")?;
        let body = &rest[1..end];
        rest = &rest[end + 1..];
        if body.starts_with('?') || body.starts_with('!') {
            continue;
        }
        out.push(parse_tag(body)?);
    }
    if !rest.trim().is_empty() {
        bail!("trailing text {:?}", rest.trim());
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq)]
pub struct XmlLayer {
    pub name: String,
    pub enabled: bool,
    pub type_name: String,
    pub blend_mode: String,
    pub opacity: u32,
    pub parameters: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct XmlGroup {
    pub name: String,
    pub enabled: bool,
    pub opacity: u32,
    pub layers: Vec<XmlLayer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct XmlStack {
    pub name: String,
    pub groups: Vec<XmlGroup>,
}

fn flag(s: &str) -> Result<bool> {
    match s {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => bail!("expected true/false, got {s:?}"),
    }
}

fn opacity(s: &str) -> Result<u32> {
    let v: u32 = s.trim().parse().with_context(|| format!("opacity {s:?}"))?;
    if v > 255 {
        bail!("opacity {v} > 255");
    }
    Ok(v)
}

pub fn parse_stack(text: &str) -> Result<XmlStack> {
    let tags = tags(text)?;
    let mut it = tags.iter();
    let first = it.next().context("empty file")?;
    if first.name != "stack" || first.closing || first.self_closing {
        bail!("expected <stack>, found <{}>", first.name);
    }
    let mut stack = XmlStack { name: first.req("name")?.to_string(), groups: Vec::new() };
    let mut closed = false;
    for t in it {
        if closed {
            bail!("<{}> after </stack>", t.name);
        }
        match (t.name, t.closing) {
            ("stack", true) => closed = true,
            ("group", false) => {
                let group = XmlGroup {
                    name: t.req("name")?.to_string(),
                    enabled: flag(t.req("enabled")?)?,
                    opacity: opacity(t.req("opacity")?)?,
                    layers: Vec::new(),
                };
                stack.groups.push(group);
            }
            ("group" | "layer", true) => {}
            ("layer", false) => {
                let group = stack.groups.last_mut().context("<layer> outside a <group>")?;
                group.layers.push(XmlLayer {
                    name: t.req("name")?.to_string(),
                    enabled: flag(t.req("enabled")?)?,
                    type_name: t.req("type")?.to_string(),
                    blend_mode: t.req("blendmode")?.to_string(),
                    opacity: opacity(t.req("opacity")?)?,
                    parameters: Vec::new(),
                });
            }
            ("parameter", false) if t.self_closing => {
                let layer = stack
                    .groups
                    .last_mut()
                    .and_then(|g| g.layers.last_mut())
                    .context("<parameter> outside a <layer>")?;
                layer.parameters.push((t.req("name")?.to_string(), t.req("value")?.to_string()));
            }
            (name, closing) => bail!("unexpected <{}{name}>", if closing { "/" } else { "" }),
        }
    }
    if !closed {
        bail!("missing </stack>");
    }
    Ok(stack)
}
