use std::collections::BTreeSet;

use anyhow::{Result, bail};

use crate::author::{escape, json};
use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::fe::{FrontEnd, credits};
use crate::game::loc::Localization;

pub const DP: &str = "dp_ava_credits_data";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub title: String,
    pub entries: Vec<(String, String)>,
}

pub fn sections(text: &str) -> Result<Vec<Section>> {
    let root = json::parse(text, &crate::author::limits::JSON)
        .map_err(|e| anyhow::anyhow!("credits: {}: {}", e.pos, e.message))?;
    let obj = |n: &json::Node, allowed: &[&str]| -> Result<Vec<(String, json::Node)>> {
        let json::Value::Object(m) = &n.value else { bail!("credits: {}: not an object", n.pos) };
        if let Some((k, _)) = m.iter().find(|(k, _)| !allowed.contains(&k.name.as_str())) {
            bail!("credits: {}: unknown member `{}`", k.pos, k.name);
        }
        Ok(m.iter().map(|(k, v)| (k.name.clone(), v.clone())).collect())
    };
    let get = |m: &[(String, json::Node)], k: &str| m.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    let string = |n: Option<json::Node>, what: &str| -> Result<String> {
        match n.map(|n| n.value) {
            Some(json::Value::String(s)) if !s.is_empty() && s.chars().count() <= 120 => Ok(s),
            _ => bail!("credits: `{what}` must be a string of 1 to 120 characters"),
        }
    };
    let top = obj(&root, &["format", "sections"])?;
    if get(&top, "format").map(|n| n.value) != Some(json::Value::Number(1.0)) {
        bail!("credits: `format` must be 1");
    }
    let Some(json::Value::Array(list)) = get(&top, "sections").map(|n| n.value) else {
        bail!("credits: `sections` must be an array")
    };
    let mut out = Vec::new();
    for s in &list {
        let m = obj(s, &["title", "entries"])?;
        let title = string(get(&m, "title"), "title")?;
        let Some(json::Value::Array(entries)) = get(&m, "entries").map(|n| n.value) else {
            bail!("credits: `entries` must be an array")
        };
        let mut lines = Vec::new();
        for e in &entries {
            let m = obj(e, &["name", "by"])?;
            lines.push((string(get(&m, "name"), "name")?, string(get(&m, "by"), "by")?));
        }
        out.push(Section { title, entries: lines });
    }
    Ok(out)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThirdParty {
    pub name: String,
    pub version: String,
    pub license: String,
    pub authors: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ThirdPartyList {
    pub packages: Vec<ThirdParty>,
    pub vendored: Vec<ThirdParty>,
    pub not_shipped: Vec<String>,
}

impl ThirdPartyList {
    pub fn all(&self) -> Vec<ThirdParty> {
        let mut all: Vec<ThirdParty> = self.packages.iter().chain(&self.vendored).cloned().collect();
        all.sort_by(|a, b| (a.name.to_lowercase(), &a.version).cmp(&(b.name.to_lowercase(), &b.version)));
        all
    }
}

const MAX_FIELD: usize = 512;
const MAX_AUTHORS: usize = 64;

pub fn third_party(text: &str) -> Result<ThirdPartyList> {
    let root = json::parse(text, &crate::author::limits::JSON)
        .map_err(|e| anyhow::anyhow!("third_party.json: {}: {}", e.pos, e.message))?;
    let obj = |n: &json::Node, allowed: &[&str]| -> Result<Vec<(String, json::Node)>> {
        let json::Value::Object(m) = &n.value else { bail!("third_party.json: {}: not an object", n.pos) };
        if let Some((k, _)) = m.iter().find(|(k, _)| !allowed.contains(&k.name.as_str())) {
            bail!("third_party.json: {}: unknown member `{}`", k.pos, k.name);
        }
        Ok(m.iter().map(|(k, v)| (k.name.clone(), v.clone())).collect())
    };
    let get = |m: &[(String, json::Node)], k: &str| m.iter().find(|(n, _)| n == k).map(|(_, v)| v.value.clone());
    let string = |v: Option<json::Value>, what: &str| -> Result<String> {
        match v {
            Some(json::Value::String(s)) if !s.is_empty() && s.chars().count() <= MAX_FIELD => Ok(s),
            _ => bail!("third_party.json: `{what}` must be a string of 1 to {MAX_FIELD} characters"),
        }
    };
    let array = |v: Option<json::Value>, what: &str| -> Result<Vec<json::Node>> {
        match v {
            Some(json::Value::Array(a)) => Ok(a),
            _ => bail!("third_party.json: `{what}` must be an array"),
        }
    };
    let top = obj(&root, &["format", "generated_by", "packages", "vendored", "not_shipped"])?;
    if get(&top, "format") != Some(json::Value::Number(1.0)) {
        bail!("third_party.json: `format` must be 1");
    }
    string(get(&top, "generated_by"), "generated_by")?;
    let mut list = ThirdPartyList::default();
    let row = |p: &json::Node| -> Result<ThirdParty> {
        let m = obj(p, &["name", "version", "license", "authors"])?;
        let authors = array(get(&m, "authors"), "authors")?;
        if authors.len() > MAX_AUTHORS {
            bail!("third_party.json: {}: more than {MAX_AUTHORS} authors", p.pos);
        }
        Ok(ThirdParty {
            name: string(get(&m, "name"), "name")?,
            version: string(get(&m, "version"), "version")?,
            license: string(get(&m, "license"), "license")?,
            authors: authors.into_iter().map(|a| string(Some(a.value), "authors")).collect::<Result<_>>()?,
        })
    };
    for p in array(get(&top, "packages"), "packages")? {
        list.packages.push(row(&p)?);
    }
    for p in array(get(&top, "vendored"), "vendored")? {
        list.vendored.push(row(&p)?);
    }
    for n in array(get(&top, "not_shipped"), "not_shipped")? {
        let m = obj(&n, &["package", "reason"])?;
        string(get(&m, "reason"), "reason")?;
        list.not_shipped.push(string(get(&m, "package"), "package")?);
    }
    Ok(list)
}

fn authors_line(authors: &[String]) -> String {
    let names: Vec<String> = authors
        .iter()
        .map(|a| match a.find('<') {
            Some(i) if a.trim_end().ends_with('>') => a[..i].trim().to_string(),
            _ => a.trim().to_string(),
        })
        .filter(|a| !a.is_empty())
        .collect();
    names.join(", ")
}

// PORT(choice): the authors' lines wrap at 60 characters, by count: the roll's two fields (`txtA` / `txtB` of
// `views/activityfeedcredits.gfx`, DefineEditText 5 / 6) are 21 365 twips (1 068 px) wide with a 30-px font (`h` 600),
// about 70 characters of DIN; a wrap by the font's advances against the field's width would replace the count; these
// lines are the port's, the disc's `credits.txt` has none of them.
const ROLL_LINE: usize = 60;

fn wrap(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for piece in text.split_inclusive(", ") {
        match out.last_mut() {
            Some(l) if l.chars().count() + piece.trim_end().chars().count() <= ROLL_LINE => l.push_str(piece),
            _ => out.push(piece.to_string()),
        }
    }
    out.into_iter().map(|l| l.trim_end().to_string()).collect()
}

pub fn port_lines(
    sections: &[Section],
    third_party: &[ThirdParty],
    glyphs: Option<&BTreeSet<char>>,
    loc: &mut Localization,
) -> Vec<String> {
    let mut out = vec![
        format!("<font size=\"40\" color=\"#B8F3FF\"><b>{}</b></font>", escape(&loc.text("TXT_AVA_CREDITS_TITLE"))),
        String::new(),
        escape(&loc.text("TXT_AVA_CREDITS_GAME")),
        escape(&loc.text("TXT_AVA_CREDITS_PORT")),
        String::new(),
    ];
    for s in sections {
        out.push(String::new());
        out.push(format!("<font size=\"36\" color=\"#6EB7C6\"><b>{}</b></font>", escape(&loc.text(&s.title))));
        out.push(String::new());
        out.extend(s.entries.iter().map(|(name, by)| format!("{} - {}", escape(name), escape(by))));
    }
    if !third_party.is_empty() {
        let drawn = |s: &str| -> String {
            s.chars().map(|c| if glyphs.is_none_or(|g| g.contains(&c) || c == ' ') { c } else { '?' }).collect()
        };
        out.push(String::new());
        let heading = loc.text("TXT_AVA_CREDITS_THIRD_PARTY");
        out.push(format!("<font size=\"36\" color=\"#6EB7C6\"><b>{}</b></font>", escape(&heading)));
        out.push(String::new());
        out.push(escape(&loc.text("TXT_AVA_CREDITS_NOTICES")));
        out.push(String::new());
        for p in third_party {
            out.push(escape(&drawn(&format!("{} {} - {}", p.name, p.version, p.license))));
            out.extend(wrap(&authors_line(&p.authors)).iter().filter(|l| !l.is_empty()).map(|l| escape(&drawn(l))));
        }
    }
    out.extend([String::new(), String::new()]);
    out
}

pub fn build(fe: &mut FrontEnd, cx: &mut Ctx, loc: &mut Localization) -> Value {
    let a = &fe.ava;
    let lines = port_lines(&a.credits, &a.third_party.all(), a.glyphs.as_ref(), loc);
    let mut items: Vec<Value> = lines.iter().map(|l| Value::str(l)).collect();
    if let Some(text) = fe.credits_text.as_deref() {
        items.extend(credits::lines(text).into_iter().map(|l| Value::str(&credits::line(loc, l))));
    }
    Value::Object(cx.new_array(items))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_port_credits_read_and_are_checked() {
        let text = crate::game::port::FILES.iter().find(|(p, _)| *p == "credits.json").unwrap().1;
        let s = sections(text).unwrap();
        assert!(s.iter().all(|x| x.title.starts_with("TXT_AVA_") && !x.entries.is_empty()));
        assert!(sections(r#"{"format": 1, "sections": [], "x": 1}"#).is_err());
        assert!(sections(r#"{"format": 1, "sections": [{"title": "T", "entries": [{"name": "a"}]}]}"#).is_err());
        assert!(sections(r#"{"format": 2, "sections": []}"#).is_err());
        let mut loc = Localization::empty(crate::game::loc::Language::English);
        let p = |name: &str, authors: &[&str]| ThirdParty {
            name: name.into(),
            version: "1.0.0".into(),
            license: "MIT OR Apache-2.0".into(),
            authors: authors.iter().map(|a| a.to_string()).collect(),
        };
        let tp = [p("a", &["Ann <ann@example.org>", "Bo \u{d8}rsted"]), p("b", &[])];
        let glyphs: BTreeSet<char> = ('!'..='~').collect();
        let lines = port_lines(&s, &tp, Some(&glyphs), &mut loc);
        assert!(lines[0].starts_with("<font size=\"40\""));
        assert!(lines.iter().any(|l| l == "SSX-Library - GlitcherOG"));
        let at = lines.iter().position(|l| l == "a 1.0.0 - MIT OR Apache-2.0").unwrap();
        assert_eq!(lines[at + 1], "Ann, Bo ?rsted");
        assert_eq!(lines[at + 2], "b 1.0.0 - MIT OR Apache-2.0");
        let long: Vec<String> = (0..12).map(|i| format!("Author Number {i}")).collect();
        assert!(wrap(&authors_line(&long)).iter().all(|l| l.chars().count() <= ROLL_LINE));
    }

    fn locked(lock: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let (mut name, mut version) = (None, None);
        for line in lock.lines().chain(std::iter::once("[[package]]")) {
            if line == "[[package]]" {
                if let (Some(n), Some(v)) = (name.take(), version.take()) {
                    out.insert(format!("{n} {v}"));
                }
            } else if let Some(n) = line.strip_prefix("name = \"").and_then(|r| r.strip_suffix('"')) {
                name = Some(n.to_string());
            } else if let Some(v) = line.strip_prefix("version = \"").and_then(|r| r.strip_suffix('"')) {
                version = Some(v.to_string());
            }
        }
        out
    }

    #[test]
    fn the_third_party_list_and_the_notices_follow_cargo_lock() {
        let text = crate::game::port::FILES.iter().find(|(p, _)| *p == "third_party.json").unwrap().1;
        let list = third_party(text).unwrap();
        let lock = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock")).unwrap();
        let lock = locked(&lock);
        let notices =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/THIRD_PARTY_NOTICES.txt"))
                .unwrap();
        let stale = "the list is stale: run Engine/tools/notices/notices.py";
        for p in &list.packages {
            let key = format!("{} {}", p.name, p.version);
            assert!(lock.contains(&key), "{key} is not in Cargo.lock: {stale}");
            assert!(notices.contains(&format!("\n--- {key} ---\n")), "{key} has no notice: {stale}");
        }
        let fonts = include_str!("../../../../../../assets/fonts/fonts.toml");
        let value = |key: &str| -> Vec<&str> {
            let prefix = format!("{key} = \"");
            fonts.lines().filter_map(|l| l.strip_prefix(prefix.as_str())?.strip_suffix('"')).collect()
        };
        let manifest: BTreeSet<String> =
            value("family").iter().zip(value("version")).map(|(f, v)| format!("{f} {v}")).collect();
        let vendored: BTreeSet<String> = list.vendored.iter().map(|p| format!("{} {}", p.name, p.version)).collect();
        assert_eq!(vendored, manifest, "{stale}");
        for key in &vendored {
            assert!(notices.contains(&format!("\n--- {key} ---\n")), "{key} has no notice: {stale}");
        }
        let known: BTreeSet<String> =
            list.packages.iter().map(|p| format!("{} {}", p.name, p.version)).chain(list.not_shipped).collect();
        let unknown: Vec<&String> = lock.iter().filter(|k| !known.contains(*k)).collect();
        assert!(unknown.is_empty(), "Cargo.lock has packages the list does not know: {unknown:?}: {stale}");
        assert!(
            third_party(
                r#"{"format": 1, "generated_by": "x", "packages": [{"name": "a"}], "vendored": [], "not_shipped": []}"#
            )
            .is_err()
        );
    }
}
