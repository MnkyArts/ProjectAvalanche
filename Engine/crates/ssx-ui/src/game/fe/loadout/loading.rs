use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use ssx_formats::attrib::collection::{self, FieldData};
use ssx_formats::attrib::{Schema, Value as AttribValue, Vault, fnv1};

use ssx_assets::career::random;

use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::fe::{FeEnv, FrontEnd};
use crate::game::loc::Localization;

const CLASS_TIP: u32 = 0x479D_9D89;
const CLASS_CHOOSERS: u32 = 0xD4C9_DD1E;
const DEFAULT: u32 = fnv1(b"default");
const FILLERS: u32 = 0xA658_6CCF;
const TIPS: u32 = 0x24E4_1CF5;
const FORCED: u32 = 0xA56A_79D5;
const CHOOSER_EXPLORE: u32 = 0x228A_D516;
const CHOOSER_GLOBAL: u32 = 0x506F_64D9;
const CHOOSER_TYPES_0_2: u32 = 0x8A82_6BC6;
const CHOOSER_TYPES_3_4: u32 = 0x8352_B9C1;
const CHOOSER_TYPES_5_7: u32 = 0x3077_795A;
pub const CONTEXT_TIPS: usize = 1;
pub const TIP_COUNT: usize = 10;

#[derive(Clone, Debug, Default)]
pub struct Tip {
    pub key: u32,
    pub text: String,
    pub title: String,
    pub image: String,
    pub controller: Vec<String>,
    pub global_event: Vec<String>,
    pub screen_type: i64,
    pub pro_tip: String,
}

#[derive(Clone, Debug, Default)]
pub struct Chooser {
    pub tips: Vec<u32>,
    pub forced: Vec<u32>,
    pub fillers: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Tips {
    pub tips: Vec<Tip>,
    pub chooser: Vec<u32>,
    pub fillers: Vec<String>,
    pub choosers: std::collections::BTreeMap<u32, Chooser>,
}

fn refs(d: &FieldData) -> Vec<u32> {
    match d {
        FieldData::Single(AttribValue::Ref(r)) => vec![r.collection],
        FieldData::Array(vs) => {
            vs.iter().filter_map(|v| if let AttribValue::Ref(r) = v { Some(r.collection) } else { None }).collect()
        }
        _ => Vec::new(),
    }
}

fn texts(d: &FieldData) -> Vec<String> {
    let vals: Vec<&AttribValue> = match d {
        FieldData::Single(v) => vec![v],
        FieldData::Array(vs) => vs.iter().collect(),
    };
    vals.into_iter().filter_map(|v| if let AttribValue::Text(Some(t)) = v { Some(t.clone()) } else { None }).collect()
}

pub struct AttribDb {
    pub cols: Vec<collection::Collection>,
    pub decoded: Vec<collection::DecodedCollection>,
    pub local: HashMap<(u32, u32), usize>,
}

impl AttribDb {
    pub fn load(big_root: &Path) -> Result<Self> {
        Self::from_files(&crate::fs::Disc::new(big_root)?)
    }

    pub fn from_files(files: &dyn crate::fs::FileSource) -> Result<Self> {
        let read = |n: &str| {
            let path = format!("data/xenon/attribdb/{n}");
            files.read(&path).with_context(|| format!("reading {path}"))
        };
        let (svlt, sbin) = (read("attribdb_schema.vlt")?, read("attribdb_schema.bin")?);
        let schema = Schema::from_vault(&Vault::parse(&svlt, &sbin)?).context("the schema")?;
        let (vlt, bin) = (read("attribdb.vlt")?, read("attribdb.bin")?);
        let v = Vault::parse(&vlt, &bin)?;
        let cols = collection::collections(&v)?;
        let decoded = cols.iter().map(|c| c.decode(&v, &schema)).collect::<ssx_formats::Result<Vec<_>>>()?;
        let local = collection::index(&cols);
        Ok(AttribDb { cols, decoded, local })
    }

    pub fn fields(&self, i: usize) -> Result<Vec<(u32, &collection::Field)>> {
        Ok(collection::with_inherited(&self.cols, &self.decoded, &self.local, i)?)
    }
}

impl Tips {
    pub fn load(big_root: &Path) -> Result<Self> {
        Self::from_db(&AttribDb::load(big_root)?)
    }

    pub fn from_db(db: &AttribDb) -> Result<Self> {
        let mut out = Tips::default();
        for (i, c) in db.cols.iter().enumerate() {
            if c.class != CLASS_TIP && c.class != CLASS_CHOOSERS {
                continue;
            }
            let fields = db.fields(i)?;
            let get = |k: u32| fields.iter().find(|(_, f)| f.key == k).map(|(_, f)| texts(&f.data)).unwrap_or_default();
            if c.class == CLASS_CHOOSERS {
                let list =
                    |k: u32| fields.iter().find(|(_, f)| f.key == k).map(|(_, f)| refs(&f.data)).unwrap_or_default();
                let ch = Chooser { tips: list(TIPS), forced: list(FORCED), fillers: get(FILLERS) };
                if c.key == DEFAULT {
                    out.fillers = ch.fillers.clone();
                    out.chooser = ch.tips.clone();
                }
                out.choosers.insert(c.key, ch);
                continue;
            }
            let one = |k: u32| get(k).into_iter().next().unwrap_or_default();
            let screen_type = fields
                .iter()
                .find(|(_, f)| f.key == 0x8250_9184)
                .and_then(|(_, f)| match &f.data {
                    FieldData::Single(AttribValue::Int32(x)) => Some(i64::from(*x)),
                    _ => None,
                })
                .unwrap_or(0);
            out.tips.push(Tip {
                key: c.key,
                text: one(0x3A6C_C2B8),
                title: one(0xDCE0_E62D),
                image: one(0x838A_6D86).replace("%s", "xbox"),
                controller: get(0xF637_FFB1),
                global_event: get(0xF837_527E),
                screen_type,
                pro_tip: one(0x7834_8D7C),
            });
        }
        Ok(out)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Loading {
    pub tips: Tips,
    pub shown: f64,
    pub active: bool,
    pub list: Option<Vec<TipEntry>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TipEntry {
    Tip { key: u32, filler: Option<String> },
    Filler(String),
}

pub fn activate(fe: &mut FrontEnd, env: &mut FeEnv, view_model: &str) {
    env.values.set_bool(0x9C, true);
    env.values.set_bool(0x1AE, true);
    env.values.set_bool(0x1B6, true);
    fe.note(format!("loading screen ({view_model}): Input_Unlock(1)"));
    fe.busy.release(env, crate::game::fe::busy::PAUSE_LEAVE);
    env.values.set_bool(0x57, false);
    fe.loading.active = true;
    fe.loading.shown = 0.0;
    fe.loading.list = None;
    fe.publish.push("dp_loadingscreen_data".into());
}

pub fn update(fe: &mut FrontEnd, _env: &mut FeEnv, dt: f64) {
    if fe.loading.active {
        fe.loading.shown += dt;
    }
}

fn set_text(cx: &mut Ctx, loc: &mut Localization, o: &Value, field: &str, text: &str) {
    let t = if text.is_empty() { String::new() } else { loc.text(text) };
    let _ = cx.set_member(o, field, Value::str(&t));
}

fn filler(tips: &Tips, chooser: u32) -> Option<String> {
    let list = &tips.choosers.get(&chooser)?.fillers;
    if list.is_empty() {
        return None;
    }
    list.get(random::below(list.len() as u32) as usize).cloned()
}

fn tip(tips: &Tips, key: u32, chooser: u32) -> TipEntry {
    let own = tips.tips.iter().find(|t| t.key == key).is_some_and(|t| !t.pro_tip.is_empty());
    TipEntry::Tip { key, filler: if own { None } else { filler(tips, chooser) } }
}

fn of_chooser(tips: &Tips, chooser: u32) -> Option<TipEntry> {
    let list = &tips.choosers.get(&chooser)?.tips;
    if list.is_empty() {
        return None;
    }
    let key = list[random::below(list.len() as u32) as usize];
    Some(tip(tips, key, chooser))
}

pub fn choose(fe: &FrontEnd) -> (Vec<TipEntry>, Vec<String>) {
    let mut log = Vec::new();
    let tips = &fe.loading.tips;
    let mut out = Vec::new();
    let drop = fe.current_drop;
    for &k in tips.choosers.get(&drop).map(|c| c.forced.as_slice()).unwrap_or_default() {
        out.push(tip(tips, k, drop));
    }
    let event = fe.career.event(drop);
    let mut count = out.len();
    let mut last = drop;
    let ends = tips.choosers.get(&DEFAULT).is_some_and(|c| !c.tips.is_empty());
    while ends && count < CONTEXT_TIPS {
        let r = random::range(0.0, 1.0);
        if r < 1.0 {
            let (entry, ret) = pick(fe, event, r);
            log.push(format!("draw {r:.4} → {entry:?}, chooser {ret:#010x}"));
            last = ret;
            out.extend(entry);
        }
        if last != 0 {
            count += 1;
        }
    }
    while count < TIP_COUNT {
        out.push(TipEntry::Filler(filler(tips, last).unwrap_or_default()));
        count += 1;
    }
    log.push(format!("fillers of chooser {last:#010x}"));
    (out, log)
}

fn pick(fe: &FrontEnd, event: Option<&ssx_assets::career::data::Event>, r: f32) -> (Option<TipEntry>, u32) {
    let tips = &fe.loading.tips;
    let with = |chooser: u32| of_chooser(tips, chooser).map_or((None, 0), |e| (Some(e), chooser));
    if r < 0.2 {
        let rider = fe.equipment.inv.as_ref().map_or(0, |i| i.rider);
        if tips.tips.iter().any(|t| t.key == rider) { (Some(tip(tips, rider, DEFAULT)), DEFAULT) } else { (None, 0) }
    } else if r < 0.3 {
        match event.map(|e| fe.career.peak_and_range(e).0).filter(|p| tips.tips.iter().any(|t| t.key == *p)) {
            Some(peak) => (Some(tip(tips, peak, DEFAULT)), DEFAULT),
            None => (None, 0),
        }
    } else if r < 0.6 {
        match fe.game_mode {
            0 => with(CHOOSER_EXPLORE),
            1 => with(DEFAULT),
            3 => with(CHOOSER_GLOBAL),
            _ => (None, 0),
        }
    } else if r < 0.9 {
        match event.map(|e| e.event_type) {
            Some(0..=2) => with(CHOOSER_TYPES_0_2),
            Some(3 | 4) => with(CHOOSER_TYPES_3_4),
            Some(5..=7) => with(CHOOSER_TYPES_5_7),
            Some(_) => with(DEFAULT),
            None => (None, 0),
        }
    } else {
        with(DEFAULT)
    }
}

pub fn build(fe: &mut FrontEnd, cx: &mut Ctx, loc: &mut Localization) -> Value {
    if fe.loading.list.is_none() {
        let (list, log) = choose(fe);
        let first = list.first().cloned();
        fe.loading.list = Some(list);
        fe.note(format!("loading tips (drop {:#010x}): {}; first {first:?}", fe.current_drop, log.join("; ")));
    }
    let tips = &fe.loading.tips;
    let pro = |cx: &mut Ctx, loc: &mut Localization, o: &Value, body: &str| {
        set_text(cx, loc, o, "tipProTip", body);
        set_text(cx, loc, o, "tipProTipTitle", "TXT_PRO_TIP_TITLE");
    };
    let mut items = Vec::new();
    for e in fe.loading.list.as_deref().unwrap_or_default() {
        let o = Value::Object(cx.new_object());
        match e {
            TipEntry::Tip { key, filler } => {
                let t = tips.tips.iter().find(|t| t.key == *key).cloned().unwrap_or_default();
                set_text(cx, loc, &o, "tipText", &t.text);
                set_text(cx, loc, &o, "tipTitle", &t.title);
                let _ = cx.set_member(&o, "tipPrimaryImage", Value::str(&t.image));
                for (k, c) in t.controller.iter().enumerate() {
                    set_text(cx, loc, &o, &format!("controllerText{k}"), c);
                }
                for (k, c) in t.global_event.iter().enumerate() {
                    set_text(cx, loc, &o, &format!("globaleventText{k}"), c);
                }
                let _ = cx.set_member(&o, "screenType", Value::Number(t.screen_type as f64));
                match filler {
                    Some(f) => pro(cx, loc, &o, f),
                    None if !t.pro_tip.is_empty() => pro(cx, loc, &o, &t.pro_tip),
                    None => {}
                }
            }
            TipEntry::Filler(f) if !f.is_empty() => pro(cx, loc, &o, f),
            TipEntry::Filler(_) => {}
        }
        items.push(o);
    }
    let out = Value::Object(cx.new_object());
    let arr = Value::Object(cx.new_array(items));
    let _ = cx.set_member(&out, "tipArray", arr);
    out
}
