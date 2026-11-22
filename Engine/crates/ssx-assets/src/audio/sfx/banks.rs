use super::*;

#[derive(Clone, Debug, PartialEq)]
pub struct TrackAudioInfo {
    pub region: String,
    pub dd_region: Option<String>,
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
            dd_region: None,
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
    use ssx_formats::attrib::Value;
    crate::vault_db::with_all_db(big_root, |db| {
        let want = fnv1(&format!("{track}_track"));
        let mut peaks = Vec::new();
        for (key, _) in db.collections_of_all(fnv1("ge_drop")) {
            let d = db.fields(fnv1("ge_drop"), key)?;
            let on_track = d.iter().any(|f| {
                f.key == fnv1("Track") && matches!(single(&f.data), Some(Value::Ref(r)) if r.collection == want)
            });
            if let Some(peak) = d.iter().find(|f| f.key == 0x17E8_41D0).and_then(|f| single_text(&f.data))
                && on_track
                && !peaks.contains(&peak)
            {
                peaks.push(peak);
            }
        }
        let [peak] = peaks.as_slice() else { anyhow::bail!("{track}: the drops name the peaks {peaks:?}") };
        let class_ref = |class: &str, key: u32, field: u32| -> Result<u32> {
            let fields = db.fields(fnv1(class), key).with_context(|| format!("{class}/{key:#010x} missing"))?;
            fields
                .iter()
                .find_map(|f| match single(&f.data) {
                    Some(Value::Ref(r)) if f.key == field => Some(r.collection),
                    _ => None,
                })
                .with_context(|| format!("{class}/{key:#010x}: no field {field:#010x}"))
        };
        let peak_key = fnv1(peak);
        let range = class_ref("peak", peak_key, 0xAB6B_2547)?;
        let range_info = class_ref("range", range, 0x220B_386B)?;
        let peak_info = class_ref("peak", peak_key, 0xC1FF_08AD)?;
        let info = |key: u32| -> Result<Vec<&ssx_formats::attrib::Field>> {
            db.fields(0x2306_91A5, key).with_context(|| format!("audio_game_info/{key:#010x} missing"))
        };
        let text = |fields: &[&ssx_formats::attrib::Field], key: u32| -> Result<String> {
            fields
                .iter()
                .find(|f| f.key == key)
                .and_then(|f| single_text(&f.data))
                .with_context(|| format!("audio_game_info: no text {key:#010x}"))
        };
        let (r, p) = (info(range_info)?, info(peak_info)?);
        let ambience_index = r
            .iter()
            .find_map(|f| match single(&f.data) {
                Some(Value::Int32(v)) if f.key == 0x107E_2BC8 => Some(*v),
                _ => None,
            })
            .context("audio_game_info: no ambience index")?;
        let region_id = r
            .iter()
            .find_map(|f| match single(&f.data) {
                Some(Value::Int32(v)) if f.key == 0x064E_15D1 => Some(*v),
                _ => None,
            })
            .unwrap_or(-1);
        let info_ref = |fields: &[&ssx_formats::attrib::Field], key: u32| {
            fields.iter().find_map(|f| match single(&f.data) {
                Some(Value::Ref(r)) if f.key == key => Some(r.collection),
                _ => None,
            })
        };
        Ok(TrackAudioInfo {
            region: text(&r, 0x882D_7954)?,
            dd_region: r.iter().find(|f| f.key == 0x6A28_B46E).and_then(|f| single_text(&f.data)),
            ambience_index,
            reverb_large: text(&p, 0xCF15_CB9A)?,
            reverb_small: text(&p, 0xB5BE_FF86)?,
            region_id,
            dd_rider: info_ref(&p, 0xD117_F93D).unwrap_or(0),
            dd_gear: info_ref(&p, 0x885B_4E6D).unwrap_or(0),
        })
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct RiderAudioInfo {
    pub signature: String,
    pub foley: String,
    pub speech_id: i32,
    pub speech_name: String,
    pub key: u32,
}

impl RiderAudioInfo {
    pub fn generic() -> Self {
        Self { signature: "Generic".into(), foley: "big".into(), speech_id: 0, speech_name: "NoBody".into(), key: 0 }
    }
}

pub fn rider_audio_info(big_root: &Path, character: &str) -> Result<RiderAudioInfo> {
    use crate::vault_db::{fnv1, single, single_text};
    use ssx_formats::attrib::Value;
    let voice = crate::content::GameContent::of(big_root).voice_of(character);
    let name = match voice.as_str() {
        "travisrice" => "travis",
        "ty" => "tyler",
        n => n,
    };
    crate::vault_db::with_all_db(big_root, |db| {
        let key = fnv1(name);
        let fields = db.fields(fnv1("character"), key).with_context(|| format!("character/{name} missing"))?;
        let info = fields
            .iter()
            .find_map(|f| match single(&f.data) {
                Some(Value::Ref(r)) if f.key == 0x0754_3721 => Some(r.collection),
                _ => None,
            })
            .with_context(|| format!("character/{name}: no audio_game_info"))?;
        let fields = db.fields(0x2306_91A5, info).with_context(|| format!("audio_game_info/{info:#010x} missing"))?;
        let text = |key: u32| {
            fields
                .iter()
                .find(|f| f.key == key)
                .and_then(|f| single_text(&f.data))
                .with_context(|| format!("audio_game_info/{info:#010x}: no text {key:#010x}"))
        };
        let speech_id = fields
            .iter()
            .find_map(|f| match single(&f.data) {
                Some(Value::Int32(v)) if f.key == 0xF70C_CEC7 => Some(*v),
                _ => None,
            })
            .unwrap_or(0);
        Ok(RiderAudioInfo {
            signature: text(0xC7A9_471C)?,
            foley: text(0x5D70_3FE2)?,
            speech_id,
            speech_name: text(0xB939_B740)?,
            key: info,
        })
    })
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
    nis_descr: Option<&str>,
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
    let trick_set = drawn("SfxTrickBank")?;
    // TODO(guess): the port draws and loads the hot-swap bank once with the level's banks; the game draws it at the
    // race start (0x9CEE015B → 0x830942D0: state 3 → 0x830940A0 in 0x83095D70) into slot +0x3C when that slot is
    // empty, and the drop's reset stage empties it (audio state 5 from 0x823F8DE0 with 0x6A2C1D32 → 0x83087F28 →
    // 0x83094808), so a reset's next race start draws a new one; the port's engine cannot unload and swap a bank
    // mid-level (`SfxEngine` adds banks only).
    let hotswap = drawn("SfxAwesomeHotswapBank")?;
    let fixed = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut v = fixed(&["board_ice", "board_packed", "board_powder", "board_rock", "grind_sweetner"]);
    v.extend(fixed(&["collisions", "ant_tags", "foley_common"]));
    v.push(nis_bank(big_root, nis_descr)?);
    v.push(by_descr("SfxGearCharacterFoleyBank", &rider.foley)?);
    v.push("boost_awesome".into());
    v.push(hotswap);
    v.extend(fixed(&["tricky", "hud", "big_air"]));
    v.push(trick_set);
    v.extend(fixed(&["ambience_ext", "flares", "avalanche", "wingsuit_a", "iceaxe"]));
    v.push(by_descr("SfxTrickSignatureBank", &rider.signature)?);
    v.push(by_descr("SfxEnvironmentWindBank", &info.region)?);
    Ok(v)
}

pub fn dd_nis_descr(block: &speech::launch::LaunchBlock, info: &TrackAudioInfo, language: &str) -> Option<String> {
    let w = &block.campaign;
    if !(w.dd && block.dd_hold) {
        return None;
    }
    let region = if w.chapter == w.chapters { "Final" } else { info.dd_region.as_deref()? };
    Some(format!("{region},{language}"))
}

fn nis_bank(big_root: &Path, descr: Option<&str>) -> Result<String> {
    const BANK: &str = "SfxAntTagsNisBank";
    const STREAM: &str = "SfxAntTagsNisStream";
    let banks = audio_assets(big_root, BANK)?;
    let of = |all: &[(String, String)], d: &str| all.iter().find(|(x, _)| x == d).map(|(_, s)| s.clone());
    if let Some(d) = descr
        && let Some(bank) = of(&banks, d)
    {
        let stream = of(&audio_assets(big_root, STREAM)?, d).with_context(|| format!("no {STREAM} \"{d}\""))?;
        anyhow::ensure!(stream == bank, "{STREAM} \"{d}\" is {stream}, not the bank's {bank}");
        return Ok(bank);
    }
    of(&banks, "ExploreDrop").with_context(|| format!("no {BANK} \"ExploreDrop\""))
}

#[cfg(test)]
mod tests {
    use super::speech::launch::{CampaignWords, GEAR_TYPES, LaunchBlock, NO_GEAR};
    use super::*;

    fn words(chapter: i32, dd: bool) -> CampaignWords {
        CampaignWords {
            chapter,
            chapters: 11,
            drops: 6,
            drop: 6,
            dd,
            before_dd: false,
            completed: false,
            region_intro: false,
            completions: 0,
            failures: 0,
        }
    }

    #[test]
    fn a_deadly_descent_with_its_rider_takes_its_regions_nis_bank() {
        let (zoe, elise) = (0x405D_C1D5, 0x1234);
        let info = TrackAudioInfo { dd_region: Some("Rockies".into()), ..TrackAudioInfo::generic() };
        let block = |chapter, dd, rider| {
            LaunchBlock::new(words(chapter, dd), [false; 7], [0; GEAR_TYPES], rider, Some((zoe, NO_GEAR)))
        };
        assert_eq!(dd_nis_descr(&block(2, true, zoe), &info, "English").as_deref(), Some("Rockies,English"));
        assert_eq!(dd_nis_descr(&block(2, true, zoe), &info, "German").as_deref(), Some("Rockies,German"));
        assert_eq!(dd_nis_descr(&block(11, true, zoe), &info, "French").as_deref(), Some("Final,French"));
        assert_eq!(dd_nis_descr(&block(2, true, elise), &info, "English"), None, "another rider: +0x21 clear");
        assert_eq!(dd_nis_descr(&block(2, false, zoe), &info, "English"), None, "not a Deadly Descent");
        let none = TrackAudioInfo::generic();
        assert_eq!(dd_nis_descr(&block(2, true, zoe), &none, "English"), None, "a range info without the text");
    }
}
