use std::collections::BTreeMap;

use md5::{Digest, Md5};
use quick_xml::Reader;
use quick_xml::events::Event;
use ssx_assets::subset::Member;

// PORT(choice): 256 KiB, 8 levels, 4 096 files (the disc's largest list is about 3 KiB with 24 files); tuning: the
// lists of other pressings; the original has no dedicated server.
pub const MAX_LIST: usize = 256 * 1024;
const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 4096;

pub fn track_of(m: &Member) -> Option<&str> {
    let rest = m.path.strip_prefix("data/xenon/gameexplorer/")?;
    let (track, name) = rest.split_once('/')?;
    (m.archive == "data0" && !name.contains('/') && name.ends_with(".subd")).then_some(track)
}

pub fn list_member(track: &str) -> Member {
    Member { archive: "data0".into(), path: format!("data/xenon/gameexplorer/{track}/{track}_track_subd_md5.xml") }
}

pub fn parse(xml: &[u8]) -> Result<BTreeMap<String, [u8; 16]>, String> {
    parse_list(xml).map_err(|e| ssx_core::text::message(&e).into_string())
}

fn parse_list(xml: &[u8]) -> Result<BTreeMap<String, [u8; 16]>, String> {
    if xml.len() > MAX_LIST {
        return Err(format!("larger than {MAX_LIST} bytes"));
    }
    let text = std::str::from_utf8(xml.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(xml)).map_err(|_| "not UTF-8")?;
    let mut reader = Reader::from_str(text);
    let mut path: Vec<String> = Vec::new();
    let mut out = BTreeMap::new();
    let (mut name, mut hash): (Option<String>, Option<[u8; 16]>) = (None, None);
    let mut buf = String::new();
    loop {
        let event = reader.read_event().map_err(|e| format!("at byte {}: {e}", reader.buffer_position()))?;
        match event {
            Event::Start(e) => {
                if path.len() >= MAX_DEPTH {
                    return Err(format!("nested deeper than {MAX_DEPTH}"));
                }
                path.push(String::from_utf8_lossy(e.local_name().as_ref()).into_owned());
                buf.clear();
            }
            Event::End(_) => {
                let Some(tag) = path.pop() else { return Err("an end tag without a start".into()) };
                let at = path.iter().map(String::as_str).collect::<Vec<_>>();
                match (at.as_slice(), tag.as_str()) {
                    (["track", "subds", "subd"], "filename") => {
                        let file = buf.rsplit(['\\', '/']).next().unwrap_or_default().to_string();
                        if file.is_empty() || name.replace(file).is_some() {
                            return Err("a <subd> with no file name, or two".into());
                        }
                    }
                    (["track", "subds", "subd"], "md5hash") => {
                        let h = md5_hex(buf.trim()).ok_or("a <md5hash> that is not 32 hex digits")?;
                        if hash.replace(h).is_some() {
                            return Err("a <subd> with two hashes".into());
                        }
                    }
                    (["track", "subds"], "subd") => {
                        let (Some(n), Some(h)) = (name.take(), hash.take()) else {
                            return Err("a <subd> without its file name and hash".into());
                        };
                        if out.len() >= MAX_ENTRIES || out.insert(n.to_ascii_lowercase(), h).is_some() {
                            return Err("a file listed twice, or too many".into());
                        }
                    }
                    _ => {}
                }
                buf.clear();
            }
            Event::Text(t) => buf.push_str(&t.xml10_content().map_err(|e| e.to_string())?),
            Event::GeneralRef(r) => match r.resolve_char_ref().map_err(|e| e.to_string())? {
                Some(c) => buf.push(c),
                None => match r.decode().map_err(|e| e.to_string())?.as_ref() {
                    "amp" => buf.push('&'),
                    "lt" => buf.push('<'),
                    "gt" => buf.push('>'),
                    "quot" => buf.push('"'),
                    "apos" => buf.push('\''),
                    other => {
                        return Err(format!(
                            "the entity &{}; (no entities)",
                            other.chars().take(32).collect::<String>()
                        ));
                    }
                },
            },
            Event::DocType(_) => return Err("a DOCTYPE (no DTD is read)".into()),
            Event::Empty(_) | Event::CData(_) => return Err("an empty element or CDATA where none belongs".into()),
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
            Event::Eof => break,
        }
    }
    if !path.is_empty() {
        return Err("the document ends inside an element".into());
    }
    Ok(out)
}

fn md5_hex(s: &str) -> Option<[u8; 16]> {
    if s.len() != 32 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(out)
}

pub fn md5(bytes: &[u8]) -> [u8; 16] {
    Md5::digest(bytes).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "\u{FEFF}<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<track inthash=\"1\" hexstringhash=\"aa\">\n  <subds>\n    <subd>\n      <filename>c:\\build\\x\\t_terrain000_mesh_subd.subd</filename>\n      <md5hash>900150983cd24fb0d6963f7d28e17f72</md5hash>\n    </subd>\n  </subds>\n</track>\n";

    #[test]
    fn a_list_reads_and_hostile_lists_are_refused() {
        let l = parse(LIST.as_bytes()).expect("reads");
        assert_eq!(l.get("t_terrain000_mesh_subd.subd"), Some(&md5(b"abc")));
        let bad = |s: String| parse(s.as_bytes()).unwrap_err();
        assert!(bad(LIST.replace("<?xml", "<!DOCTYPE t [<!ENTITY a \"x\">]><?xml")).contains("DOCTYPE"));
        assert!(bad(LIST.replace("c:\\build", "&lol;")).contains("entity"));
        assert!(bad(LIST.replace("900150983cd24fb0d6963f7d28e17f72", "9001")).contains("32 hex"));
        assert!(bad(LIST.replace("</subd>", "</subd><subd></subd>")).contains("without"));
        assert!(bad(LIST.replace("</track>", "")).contains("ends inside"));
        assert!(bad(format!("{}{}", LIST, " ".repeat(MAX_LIST))).contains("larger"));
        assert!(bad("<a>".repeat(9) + &"</a>".repeat(9)).contains("deeper"));
        let twice = LIST.replace("</subds>", "<subd><filename>T_TERRAIN000_MESH_SUBD.SUBD</filename><md5hash>900150983cd24fb0d6963f7d28e17f72</md5hash></subd></subds>");
        assert!(bad(twice).contains("twice"));
        assert_eq!(parse(LIST.replace("c:\\build", "c:&#92;b&amp;").as_bytes()).expect("refs").len(), 1);
    }

    #[test]
    fn terrain_members_and_their_lists() {
        let m = Member::new("data0", "data/xenon/gameexplorer/alps0101/alps0101_terrain000_mesh_subd.subd").unwrap();
        assert_eq!(track_of(&m), Some("alps0101"));
        assert_eq!(list_member("alps0101").path, "data/xenon/gameexplorer/alps0101/alps0101_track_subd_md5.xml");
        assert_eq!(track_of(&Member::new("data0", "data/xenon/gameexplorer/alps0101/a.pob").unwrap()), None);
    }
}
