use super::*;

#[derive(Clone, Debug, PartialEq)]
pub struct TrackAudioInfo {
    pub region: String,
    pub ambience_index: i32,
    pub reverb_large: String,
    pub reverb_small: String,
    pub region_id: i32,
    pub dd_rider: u32,
    pub dd_gear: u32,
}

impl TrackAudioInfo {
    pub fn generic() -> Self {
        Self {
            region: "Generic".into(),
            ambience_index: 9,
            reverb_large: "africa_int_lg".into(),
            reverb_small: "africa_int_sm".into(),
            region_id: -1,
            dd_rider: 0,
            dd_gear: speech::launch::NO_GEAR,
        }
    }
}

pub fn track_audio_info(big_root: &Path, track: &str) -> Result<TrackAudioInfo> {
    use crate::vault_db::{fnv1, single, single_text};
    use ssx_formats::attrib::{Schema, Value, Vault, collection};
    let dir = big_root.join("bootcache/data/xenon/attribdb");
    let read = |stem: &str| -> Result<(Vec<u8>, Vec<u8>)> {
        let p = |ext: &str| dir.join(format!("{stem}.{ext}"));
        let v = std::fs::read(p("vlt")).with_context(|| format!("reading {}", p("vlt").display()))?;
        Ok((v, std::fs::read(p("bin"))?))
    };
    let (sv, sb) = read("attribdb_schema")?;
    let schema = Schema::from_vault(&Vault::parse(&sv, &sb)?).context("reading the schema")?;
    let (gv, gb) = read("ge_attribdb")?;
    let gvault = Vault::parse(&gv, &gb)?;
    let gcols = collection::collections(&gvault)?;
    let want = fnv1(&format!("{track}_track"));
    let mut peaks = Vec::new();
    for c in gcols.iter().filter(|c| c.class == fnv1("ge_drop")) {
        let d = c.decode(&gvault, &schema)?;
        let on_track = d
            .fields
            .iter()
            .any(|f| f.key == fnv1("Track") && matches!(single(&f.data), Some(Value::Ref(r)) if r.collection == want));
        if let Some(peak) = d.fields.iter().find(|f| f.key == 0x17E8_41D0).and_then(|f| single_text(&f.data))
            && on_track
            && !peaks.contains(&peak)
        {
            peaks.push(peak);
        }
    }
    let [peak] = peaks.as_slice() else { anyhow::bail!("{track}: the drops name the peaks {peaks:?}") };
    let (av, ab) = read("attribdb")?;
    let vault = Vault::parse(&av, &ab)?;
    let cols = collection::collections(&vault)?;
    let decoded = cols.iter().map(|c| c.decode(&vault, &schema)).collect::<ssx_formats::Result<Vec<_>>>()?;
    let index = collection::index(&cols);
    let class_ref = |class: &str, key: u32, field: u32| -> Result<u32> {
        let i = *index.get(&(fnv1(class), key)).with_context(|| format!("{class}/{key:#010x} missing"))?;
        let fields = collection::with_inherited(&cols, &decoded, &index, i)?;
        fields
            .iter()
            .find_map(|(_, f)| match single(&f.data) {
                Some(Value::Ref(r)) if f.key == field => Some(r.collection),
                _ => None,
            })
            .with_context(|| format!("{class}/{key:#010x}: no field {field:#010x}"))
    };
    let peak_key = fnv1(peak);
    let range = class_ref("peak", peak_key, 0xAB6B_2547)?;
    let range_info = class_ref("range", range, 0x220B_386B)?;
    let peak_info = class_ref("peak", peak_key, 0xC1FF_08AD)?;
    let gdecoded = gcols.iter().map(|c| c.decode(&gvault, &schema)).collect::<ssx_formats::Result<Vec<_>>>()?;
    let gindex = collection::index(&gcols);
    let info = |key: u32| -> Result<Vec<(u32, &collection::Field)>> {
        let i = *gindex.get(&(0x2306_91A5, key)).with_context(|| format!("audio_game_info/{key:#010x} missing"))?;
        Ok(collection::with_inherited(&gcols, &gdecoded, &gindex, i)?)
    };
    let text = |fields: &[(u32, &collection::Field)], key: u32| -> Result<String> {
        fields
            .iter()
            .find(|(_, f)| f.key == key)
            .and_then(|(_, f)| single_text(&f.data))
            .with_context(|| format!("audio_game_info: no text {key:#010x}"))
    };
    let (r, p) = (info(range_info)?, info(peak_info)?);
    let ambience_index = r
        .iter()
        .find_map(|(_, f)| match single(&f.data) {
            Some(Value::Int32(v)) if f.key == 0x107E_2BC8 => Some(*v),
            _ => None,
        })
        .context("audio_game_info: no ambience index")?;
    let region_id = r
        .iter()
        .find_map(|(_, f)| match single(&f.data) {
            Some(Value::Int32(v)) if f.key == 0x064E_15D1 => Some(*v),
            _ => None,
        })
        .unwrap_or(-1);
    let info_ref = |fields: &[(u32, &collection::Field)], key: u32| {
        fields.iter().find_map(|(_, f)| match single(&f.data) {
            Some(Value::Ref(r)) if f.key == key => Some(r.collection),
            _ => None,
        })
    };
    Ok(TrackAudioInfo {
        region: text(&r, 0x882D_7954)?,
        ambience_index,
        reverb_large: text(&p, 0xCF15_CB9A)?,
        reverb_small: text(&p, 0xB5BE_FF86)?,
        region_id,
        dd_rider: info_ref(&p, 0xD117_F93D).unwrap_or(0),
        dd_gear: info_ref(&p, 0x885B_4E6D).unwrap_or(0),
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct RiderAudioInfo {
    pub signature: String,
    pub foley: String,
    pub speech_id: i32,
    pub key: u32,
}

impl RiderAudioInfo {
    pub fn generic() -> Self {
        Self { signature: "Generic".into(), foley: "big".into(), speech_id: 0, key: 0 }
    }
}

pub fn rider_audio_info(big_root: &Path, character: &str) -> Result<RiderAudioInfo> {
    use crate::vault_db::{fnv1, single, single_text};
    use ssx_formats::attrib::{Schema, Value, Vault, collection};
    let voice = crate::content::GameContent::of(big_root).voice_of(character);
    let name = match voice.as_str() {
        "travisrice" => "travis",
        "ty" => "tyler",
        n => n,
    };
    let dir = big_root.join("bootcache/data/xenon/attribdb");
    let read = |stem: &str| -> Result<(Vec<u8>, Vec<u8>)> {
        let p = |ext: &str| dir.join(format!("{stem}.{ext}"));
        let v = std::fs::read(p("vlt")).with_context(|| format!("reading {}", p("vlt").display()))?;
        Ok((v, std::fs::read(p("bin"))?))
    };
    let (sv, sb) = read("attribdb_schema")?;
    let schema = Schema::from_vault(&Vault::parse(&sv, &sb)?).context("reading the schema")?;
    let (av, ab) = read("attribdb")?;
    let vault = Vault::parse(&av, &ab)?;
    let cols = collection::collections(&vault)?;
    let (gv, gb) = read("ge_attribdb")?;
    let gvault = Vault::parse(&gv, &gb)?;
    let gcols = collection::collections(&gvault)?;
    let is_char = |c: &&collection::Collection| c.class == fnv1("character") && c.key == fnv1(name);
    let decoded = match cols.iter().find(is_char) {
        Some(c) => c.decode(&vault, &schema)?,
        None => {
            gcols.iter().find(is_char).with_context(|| format!("character/{name} missing"))?.decode(&gvault, &schema)?
        }
    };
    let info = decoded
        .fields
        .iter()
        .find_map(|f| match single(&f.data) {
            Some(Value::Ref(r)) if f.key == 0x0754_3721 => Some(r.collection),
            _ => None,
        })
        .with_context(|| format!("character/{name}: no audio_game_info"))?;
    let gdecoded = gcols.iter().map(|c| c.decode(&gvault, &schema)).collect::<ssx_formats::Result<Vec<_>>>()?;
    let gindex = collection::index(&gcols);
    let i = *gindex.get(&(0x2306_91A5, info)).with_context(|| format!("audio_game_info/{info:#010x} missing"))?;
    let fields = collection::with_inherited(&gcols, &gdecoded, &gindex, i)?;
    let text = |key: u32| {
        fields
            .iter()
            .find(|(_, f)| f.key == key)
            .and_then(|(_, f)| single_text(&f.data))
            .with_context(|| format!("audio_game_info/{info:#010x}: no text {key:#010x}"))
    };
    let speech_id = fields
        .iter()
        .find_map(|(_, f)| match single(&f.data) {
            Some(Value::Int32(v)) if f.key == 0xF70C_CEC7 => Some(*v),
            _ => None,
        })
        .unwrap_or(0);
    Ok(RiderAudioInfo { signature: text(0xC7A9_471C)?, foley: text(0x5D70_3FE2)?, speech_id, key: info })
}

pub fn audio_assets(big_root: &Path, asset_type: &str) -> Result<Vec<(String, String)>> {
    let path = big_root.join("bootcache/data/xenon/audio/default_assets.xml");
    let xml = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let attr = |line: &str, k: &str| {
        let at = line.find(&format!("{k}=\""))? + k.len() + 2;
        line[at..].split('"').next().map(str::to_string)
    };
    Ok(xml
        .lines()
        .filter(|l| l.trim_start().starts_with("<Asset ") && attr(l, "assettype").as_deref() == Some(asset_type))
        .filter_map(|l| {
            let stem = Path::new(&attr(l, "path")?).file_stem()?.to_str()?.to_string();
            Some((attr(l, "descr").unwrap_or_default(), stem))
        })
        .collect())
}

// TODO(guess): "avalanche" is killed with the level's banks though the game's SfxAvalancheBank stays loaded (its index in slot 0 is overwritten by the ambience bank's, 0x83093C88 / 0x83095278), so its stopped loop would hold the slot until a later tick; settle: when the game calls the avalanche again after a restart (0x830F29D0, owner +0xB8).
pub const RESIDENT_BANKS: &[&str] = &[
    "board_ice",
    "board_packed",
    "board_powder",
    "board_rock",
    "grind_sweetner",
    "ant_tags",
    "foley_common",
    "fe_sfx",
];

pub fn riding_banks(
    big_root: &Path,
    info: &TrackAudioInfo,
    rider: &RiderAudioInfo,
    below: &mut dyn FnMut(&str, u32) -> u32,
) -> Result<Vec<String>> {
    let by_descr = |t: &str, d: &str| -> Result<String> {
        let all = audio_assets(big_root, t)?;
        all.iter().find(|(descr, _)| descr == d).map(|(_, s)| s.clone()).with_context(|| format!("no {t} \"{d}\""))
    };
    let mut drawn = |t: &str| -> Result<String> {
        let all = audio_assets(big_root, t)?;
        let i = below(t, all.len() as u32) as usize;
        all.get(i).map(|(_, s)| s.clone()).with_context(|| format!("no {t} #{i}"))
    };
    let fixed = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut v = fixed(&["board_ice", "board_packed", "board_powder", "board_rock", "grind_sweetner"]);
    v.extend(fixed(&["collisions", "ant_tags", "foley_common"]));
    v.push(by_descr("SfxAntTagsNisBank", "ExploreDrop")?);
    v.push(by_descr("SfxGearCharacterFoleyBank", &rider.foley)?);
    v.push("boost_awesome".into());
    v.push(drawn("SfxAwesomeHotswapBank")?);
    v.extend(fixed(&["tricky", "hud", "big_air"]));
    v.push(drawn("SfxTrickBank")?);
    v.extend(fixed(&["ambience_ext", "flares", "avalanche", "wingsuit_a", "iceaxe"]));
    v.push(by_descr("SfxTrickSignatureBank", &rider.signature)?);
    v.push(by_descr("SfxEnvironmentWindBank", &info.region)?);
    Ok(v)
}
