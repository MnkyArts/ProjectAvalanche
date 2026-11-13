pub mod anims;
mod ant;
pub mod audio;
pub mod blocklist;
pub mod characters;
pub mod data;
pub mod dev;
pub mod dlc;
pub mod export;
pub mod export_audio;
pub mod export_m4;
pub mod export_records;
pub mod export_rider;
mod files;
pub mod fmv;
pub mod gamemodes;
pub mod hash;
pub mod level;
pub mod m6;
pub mod maps;
pub mod markers;
pub mod models;
pub mod mods;
pub mod modtool;
pub mod objects;
pub mod particles;
pub mod presets;
mod records;
pub mod resolve;
mod script_audio;
pub mod server;
pub mod store;
pub mod textures;
pub mod title_update;
pub mod track_packages;
pub mod tricks;
pub mod tuner;
pub mod vaults;

pub use store::GameContent;

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use ssx_content::{BASE_NAMESPACE, ContentId, Kind};

pub fn segment(name: &str) -> Option<String> {
    let s: String = name.chars().map(|c| if c == ' ' { '_' } else { c.to_ascii_lowercase() }).collect();
    let ok = !s.is_empty() && s != "." && s != ".." && s.chars().all(ssx_content::id::segment_char);
    ok.then_some(s)
}

pub fn path_segments(path: &str) -> Option<String> {
    path.split('/').map(segment).collect::<Option<Vec<_>>>().map(|v| v.join("/"))
}

#[derive(Default)]
pub struct Catalogue {
    pub(super) entries: BTreeMap<ContentId, Vec<(String, String)>>,
    notes: BTreeMap<Kind, Vec<String>>,
    info: Vec<String>,
}

pub struct Ids {
    pub rows: Vec<(ContentId, String)>,
    pub notes: BTreeMap<Kind, Vec<String>>,
    pub info: Vec<String>,
    pub collisions: Vec<(ContentId, Vec<String>)>,
}

impl Catalogue {
    pub fn add(&mut self, kind: Kind, path: &str, source: impl Into<String>) {
        let source = source.into();
        self.add_record(kind, path, source.clone(), source);
    }

    pub fn add_record(&mut self, kind: Kind, path: &str, record: String, source: String) {
        match ContentId::new(BASE_NAMESPACE, kind, path) {
            Ok(id) => {
                let list = self.entries.entry(id).or_default();
                if !list.iter().any(|(r, _)| *r == record) {
                    list.push((record, source));
                }
            }
            Err(e) => self.note(kind, format!("{path:?} is not a valid id path ({e}); left out")),
        }
    }

    pub fn note(&mut self, kind: Kind, text: impl Into<String>) {
        self.notes.entry(kind).or_default().push(text.into());
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.info.push(text.into());
    }

    pub fn finish(self) -> Ids {
        let mut rows = Vec::new();
        let mut collisions = Vec::new();
        for (id, mut records) in self.entries {
            if records.len() == 1 {
                rows.push((id, records.remove(0).1));
            } else {
                collisions.push((id, records.into_iter().map(|(r, _)| r).collect()));
            }
        }
        Ids { rows, notes: self.notes, info: self.info, collisions }
    }
}

pub struct DataDirs<'a> {
    pub big: &'a Path,
    pub disc: &'a Path,
}

pub fn catalogue(dirs: &DataDirs<'_>, only: Option<Kind>) -> Result<Ids> {
    let mut c = Catalogue::default();
    let want = |k: Kind| only.is_none_or(|o| o == k);
    let vault_kinds = [
        Kind::Texture,
        Kind::Model,
        Kind::Object,
        Kind::Material,
        Kind::Character,
        Kind::Gear,
        Kind::Board,
        Kind::Track,
        Kind::Drop,
        Kind::Event,
        Kind::Lighting,
        Kind::Postfx,
    ];
    if vault_kinds.iter().any(|&k| want(k)) {
        records::add(&mut c, dirs.big, &want)?;
    }
    files::add(&mut c, dirs, &want)?;
    for k in [Kind::Sky, Kind::Marker, Kind::Map, Kind::Gamemode] {
        if want(k) {
            c.note(k, "new in the port: the base game has none");
        }
    }
    Ok(c.finish())
}

impl Ids {
    pub fn to_tsv(&self) -> String {
        let mut s = String::from("id\tkind\tsource\n");
        for (id, source) in &self.rows {
            s.push_str(&format!("{id}\t{}\t{source}\n", id.kind()));
        }
        s
    }

    pub fn count(&self, kind: Kind) -> usize {
        self.rows.iter().filter(|(id, _)| id.kind() == kind).count()
    }
}

pub fn run_ids(data: &Path, kind: Option<&str>, grep: Option<&str>, out: Option<&Path>) -> Result<()> {
    let only = match kind {
        Some(k) => Some(Kind::parse(k).ok_or_else(|| anyhow::anyhow!("unknown kind {k:?}"))?),
        None => None,
    };
    let (big, disc) = (data.join("big"), data.join("disc"));
    let start = std::time::Instant::now();
    let mut ids = catalogue(&DataDirs { big: &big, disc: &disc }, only)?;
    if let Some(g) = grep {
        ids.rows.retain(|(id, source)| id.as_str().contains(g) || source.contains(g));
    }
    let tsv = ids.to_tsv();
    let report: &mut dyn std::io::Write = match out {
        Some(path) => {
            std::fs::write(path, &tsv)?;
            &mut std::io::stdout()
        }
        None => {
            std::io::Write::write_all(&mut std::io::stdout().lock(), tsv.as_bytes())?;
            &mut std::io::stderr()
        }
    };
    writeln!(report, "{} ids in {:.1} s", ids.rows.len(), start.elapsed().as_secs_f32())?;
    for i in &ids.info {
        writeln!(report, "  {i}")?;
    }
    for k in Kind::ALL.into_iter().filter(|&k| only.is_none_or(|o| o == k)) {
        writeln!(report, "  {:<10} {:>6}", k.as_str(), ids.count(k))?;
        for n in ids.notes.get(&k).into_iter().flatten() {
            writeln!(report, "             {n}")?;
        }
    }
    writeln!(report, "collisions (left out): {}", ids.collisions.len())?;
    for (id, records) in &ids.collisions {
        writeln!(report, "  {id}: {}", records.join(" | "))?;
    }
    Ok(())
}

pub fn run_hash(data: &Path, threads: usize) -> Result<()> {
    if crate::setup::marker(data) != Ok(None) {
        anyhow::bail!(
            "{} is a setup's data folder ({}): its {} comes from the disc image; run the setup again to repair it",
            data.display(),
            crate::setup::MARKER,
            hash::INDEX_FILE
        );
    }
    let run = hash::hash_data(data, threads)?;
    let out = hash::write_index(data, &run.index)?;
    ssx_core::println!(
        "{}: {} files, {} bytes hashed in {:.2} s on {threads} threads ({:.0} MB/s)",
        out.display(),
        run.index.entries.len(),
        run.bytes,
        run.seconds,
        run.bytes as f64 / 1e6 / run.seconds.max(1e-9)
    );
    for s in &run.skipped {
        ssx_core::println!("  not a regular file, not hashed: {s}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments() {
        assert_eq!(segment("DIN Engschrift Std").as_deref(), Some("din_engschrift_std"));
        assert_eq!(segment("TXT_MENU_PLAY").as_deref(), Some("txt_menu_play"));
        assert_eq!(segment("TXT_ON\r\n"), None);
        assert_eq!(segment("a(b)"), None);
        assert_eq!(segment(".."), None);
        assert_eq!(path_segments("Worlds/Textures/Rock_02").as_deref(), Some("worlds/textures/rock_02"));
        assert_eq!(path_segments("a//b"), None);
    }

    #[test]
    fn collisions_are_kept_apart() {
        let mut c = Catalogue::default();
        c.add(Kind::Texture, "a", "x.xpr");
        c.add(Kind::Texture, "a", "x.xpr");
        c.add(Kind::Texture, "b", "y.xpr");
        c.add(Kind::Texture, "b", "z.xpr");
        c.add(Kind::Texture, "B", "w.xpr");
        c.add_record(Kind::Sound, "x", "f.csi#1".into(), "f.csi".into());
        c.add_record(Kind::Sound, "x", "f.csi#2".into(), "f.csi".into());
        let ids = c.finish();
        assert_eq!(ids.rows.len(), 1);
        assert_eq!(ids.collisions.len(), 2, "same shown source, different records");
        assert_eq!(ids.notes[&Kind::Texture].len(), 1);
    }
}
