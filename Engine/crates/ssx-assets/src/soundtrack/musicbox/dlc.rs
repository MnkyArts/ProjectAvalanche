use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ssx_formats::attrib::Value;
use ssx_formats::speech::{grf::Graffiti, rep::RepetitionPools};

use super::{Song, parse_graffiti_ids, parse_song_info, selection};
use crate::music::find_file;

pub const CLASS_AUDIO_ASSETS: u32 = 0x4876_C217;
const FIELD_TYPE: u32 = 0x966A_4025;
const FIELD_FILE: u32 = 0xE117_FFC6;
const FIELD_SAMPLE_FILES: u32 = 0xD886_172C;
const FIELD_SAMPLE_BANKS: u32 = 0x5C9D_A93D;
const FIELD_SELECTIONS: u32 = 0x3B28_0750;
const FIELD_SONG_INFO: u32 = 0x6559_3B21;
const FIELD_GRAFFITI: u32 = 0x696A_A407;
const FIELD_POOLS: u32 = 0x3C85_0274;
const BASE_SET: u32 = 0xE1D5_DD2C;

const MAX_MANIFEST_BYTES: u64 = 1 << 20;
const MAX_DATA_BYTES: u64 = 8 << 20;
pub(super) const MAX_BANK_BYTES: u64 = 1 << 20;

#[derive(Clone, Debug)]
pub struct BankFiles {
    pub sbr: PathBuf,
    pub sbs: PathBuf,
}

pub struct DlcMusic {
    pub source: String,
    pub songs: Vec<Song>,
    pub selectable: Vec<u32>,
    pub graffiti: Graffiti,
    pub selections: selection::SelectionData,
    pub pools: RepetitionPools,
    pub banks: Vec<BankFiles>,
}

pub fn music_sets(big_root: &Path) -> Vec<DlcMusic> {
    if crate::data_roots::overlays().is_empty() {
        return Vec::new();
    }
    match ssx_rules::vault_db::with_all_db(big_root, |db| collect(big_root, db)) {
        Ok(sets) => sets,
        Err(e) => {
            ssx_core::eprintln!("musicbox: DLC music sets: {}", ssx_core::text::shown_max(&format!("{e:#}"), 240));
            Vec::new()
        }
    }
}

fn collect(big_root: &Path, db: &ssx_rules::vault_db::Db) -> Result<Vec<DlcMusic>> {
    let text = |key: u32, field: u32| {
        db.field(CLASS_AUDIO_ASSETS, key, field).ok().flatten().and_then(|f| ssx_rules::vault_db::single_text(&f.data))
    };
    let kind = |key: u32| db.field(CLASS_AUDIO_ASSETS, key, FIELD_TYPE).ok().flatten().and_then(|f| raw_u32(&f.data));
    let mut assets: BTreeMap<String, String> = BTreeMap::new();
    for (key, _) in db.collections_of_all(CLASS_AUDIO_ASSETS) {
        if !matches!(kind(key), Some(1 | 2)) {
            continue;
        }
        let Some(name) = text(key, FIELD_FILE).filter(|n| !n.is_empty()) else { continue };
        let rel = format!("data/xenon/audio/{name}");
        let Some(path) = find_file(big_root, &rel) else {
            ssx_core::eprintln!("musicbox: audio manifest {} not found", ssx_core::text::shown_max(&rel, 160));
            continue;
        };
        let data = read_capped(&path, MAX_MANIFEST_BYTES)?;
        let text = std::str::from_utf8(&data).with_context(|| format!("{rel}: not UTF-8"))?;
        for (asset_type, path) in manifest(text).with_context(|| rel.clone())? {
            assets.entry(asset_type).or_insert(path);
        }
    }
    let mut out = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (key, _) in db.collections_of_all(CLASS_AUDIO_ASSETS) {
        if key == BASE_SET || kind(key) != Some(3) {
            continue;
        }
        let six: Vec<Option<String>> =
            [FIELD_SAMPLE_FILES, FIELD_SAMPLE_BANKS, FIELD_SELECTIONS, FIELD_SONG_INFO, FIELD_GRAFFITI, FIELD_POOLS]
                .iter()
                .map(|&f| text(key, f))
                .collect();
        let Some(module_type) = six[0].as_deref().filter(|t| !t.is_empty()) else { continue };
        if module_type == "MusicSampleFiles" {
            continue;
        }
        let name = db.collection_name(key);
        let resolve = |slot: usize, what: &str| -> Result<PathBuf> {
            let ty = six[slot].as_deref().with_context(|| format!("{name}: no {what} asset type"))?;
            let rel = assets.get(ty).with_context(|| format!("{name}: asset type {ty:?} is in no manifest"))?;
            find_file(big_root, rel).with_context(|| format!("{name}: {rel} not found"))
        };
        match read_set(big_root, resolve, &name) {
            Ok(set) => {
                if !seen.insert(set.source.clone()) {
                    continue;
                }
                out.push(set);
            }
            Err(e) => ssx_core::eprintln!(
                "musicbox: DLC music set {}: {}",
                ssx_core::text::shown_max(&name, 80),
                ssx_core::text::shown_max(&format!("{e:#}"), 240)
            ),
        }
    }
    Ok(out)
}

fn read_set(big_root: &Path, resolve: impl Fn(usize, &str) -> Result<PathBuf>, name: &str) -> Result<DlcMusic> {
    let module_path = resolve(0, "MusicSampleFiles")?;
    let module = read_text(&module_path, MAX_MANIFEST_BYTES)?;
    let banks = sample_banks(&module, big_root).with_context(|| module_path.display().to_string())?;
    if banks.is_empty() {
        bail!("{}: no <SampleBank> (the songs cannot play)", module_path.display());
    }
    let songs = parse_song_info(&read_capped(&resolve(3, "MusicBoxSongInfo")?, MAX_DATA_BYTES)?)?;
    let graffiti_data = read_capped(&resolve(4, "MusicGraffitiData")?, MAX_DATA_BYTES)?;
    let selectable = parse_graffiti_ids(&graffiti_data)?;
    let graffiti = Graffiti::parse(&graffiti_data)?;
    let selections = selection::parse_selections(&read_capped(&resolve(2, "MusicBoxSelections")?, MAX_DATA_BYTES)?)?;
    let pools = RepetitionPools::parse(&read_capped(&resolve(5, "MusicRepetitionPools")?, MAX_DATA_BYTES)?)?;
    Ok(DlcMusic {
        source: format!("{name} ({})", module_path.display()),
        songs,
        selectable,
        graffiti,
        selections,
        pools,
        banks,
    })
}

fn raw_u32(d: &ssx_formats::attrib::FieldData) -> Option<u32> {
    match ssx_rules::vault_db::single(d)? {
        Value::Raw { bytes, .. } if bytes.len() == 4 => {
            Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        }
        Value::UInt32(x) => Some(*x),
        _ => None,
    }
}

fn manifest(text: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for tag in crate::effects::postfx::xml::tags(text)? {
        if tag.name != "Asset" || tag.closing {
            continue;
        }
        let (Some(kind), Some(path)) = (tag.attr("assettype"), tag.attr("path")) else {
            bail!("<Asset> without assettype/path");
        };
        out.push((kind.to_string(), path.to_string()));
    }
    Ok(out)
}

fn sample_banks(text: &str, big_root: &Path) -> Result<Vec<BankFiles>> {
    let mut out = Vec::new();
    for tag in crate::effects::postfx::xml::tags(text)? {
        if tag.name != "SampleBank" || tag.closing {
            continue;
        }
        let (Some(bank), Some(stream)) = (tag.attr("bankPath"), tag.attr("streamPath")) else {
            bail!("<SampleBank> without bankPath/streamPath");
        };
        let sbr = find_file(big_root, bank).with_context(|| format!("{bank} not found"))?;
        let sbs = find_file(big_root, stream).with_context(|| format!("{stream} not found"))?;
        out.push(BankFiles { sbr, sbs });
    }
    Ok(out)
}

fn read_capped(path: &Path, max: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path).with_context(|| path.display().to_string())?;
    let meta = file.metadata().with_context(|| path.display().to_string())?;
    if !meta.is_file() || meta.len() > max {
        bail!("{}: not a file of at most {max} bytes", path.display());
    }
    let mut data = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
    file.take(max.saturating_add(1)).read_to_end(&mut data).with_context(|| path.display().to_string())?;
    if data.len() as u64 > max {
        bail!("{}: more than {max} bytes", path.display());
    }
    Ok(data)
}

fn read_text(path: &Path, max: u64) -> Result<String> {
    let data = read_capped(path, max)?;
    String::from_utf8(data).with_context(|| format!("{}: not UTF-8", path.display()))
}
