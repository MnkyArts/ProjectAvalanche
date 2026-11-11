use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use ssx_formats::attrib::collection::{self, Collection, DecodedCollection, FieldData};
use ssx_formats::attrib::{Schema, Value, Vault, fnv1};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventType {
    TrickScore = 0,
    TrickBots = 1,
    TrickGhosts = 2,
    SurviveToBottom = 3,
    SurviveToDeath = 4,
    RaceClock = 5,
    RaceGhosts = 6,
    RaceBots = 7,
    FreeRide = 8,
}

impl EventType {
    pub fn from_raw(v: u32) -> Option<Self> {
        use EventType::*;
        [TrickScore, TrickBots, TrickGhosts, SurviveToBottom, SurviveToDeath, RaceClock, RaceGhosts, RaceBots, FreeRide]
            .get(v as usize)
            .copied()
    }

    pub fn name(self) -> &'static str {
        [
            "EventType_TrickScore",
            "EventType_TrickBots",
            "EventType_TrickGhosts",
            "EventType_SurviveToBottom",
            "EventType_SurviveToDeath",
            "EventType_RaceClock",
            "EventType_RaceGhosts",
            "EventType_RaceBots",
            "EventType_FreeRide",
        ][self as usize]
    }

    pub fn kind_word(self) -> &'static str {
        if self.is_race() {
            "race"
        } else if self.is_trick() {
            "trick"
        } else if self.is_survive() {
            "survive"
        } else {
            "freeride"
        }
    }

    pub fn is_race(self) -> bool {
        matches!(self, EventType::RaceClock | EventType::RaceGhosts | EventType::RaceBots)
    }

    pub fn is_trick(self) -> bool {
        matches!(self, EventType::TrickScore | EventType::TrickBots | EventType::TrickGhosts)
    }

    pub fn is_survive(self) -> bool {
        matches!(self, EventType::SurviveToBottom | EventType::SurviveToDeath)
    }

    pub fn ranks_ascending(self) -> bool {
        self.is_race()
    }

    pub fn has_explore_ghosts(self) -> bool {
        matches!(self, EventType::RaceGhosts | EventType::TrickGhosts)
    }
}

pub fn shows_explore_ghosts(event_type: Option<EventType>, has_bots: bool) -> bool {
    !has_bots && event_type.is_some_and(EventType::has_explore_ghosts)
}

pub const CLASS_UI_DROP: u32 = fnv1(b"ui_drop");
pub const FIELD_UI_DROP_GE_DROP: u32 = 0xB9C2_CFC5;
pub const FIELD_UI_DROP_EVENT_TYPE: u32 = 0x146B_5937;
pub const FIELD_UI_DROP_SCRIPTS: u32 = 0x0E24_143E;
pub const FIELD_UI_DROP_NAME: u32 = 0xAF27_A974;
pub const FIELD_UI_DROP_REQ_EQUIPMENT: u32 = 0xD4D5_8427;
pub const FIELD_UI_DROP_AVALANCHE_SCRIPTS: u32 = 0xB1C0_1F64;
pub const FIELD_UI_DROP_FORCED_LOADOUT: u32 = 0x7E5E_ED53;
pub const FIELD_UI_DROP_SLOT_COUNTS: u32 = 0x98E9_EBCC;
pub const FIELD_UI_DROP_HUD_TITLE: u32 = 0xAFD5_EF64;
pub const FIELD_UI_DROP_HUD_BODY: u32 = 0x4375_DFCA;
pub const FIELD_UI_DROP_MESSAGE_PARAMETER: u32 = 0x0B03_FC19;
pub const FIELD_UI_DROP_TYPE: u32 = 0x555E_EBD2;
pub const FIELD_UI_DROP_PLACES: u32 = 0xC7B7_41BF;
pub const FIELD_UI_DROP_TARGETS: u32 = 0x3767_2E94;

pub fn read_pair(vlt: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    let bin = vlt.with_extension("bin");
    Ok((crate::read::read_file(vlt)?, crate::read::read_file(&bin)?))
}

fn int_of(v: &Value) -> Option<i64> {
    Some(match v {
        Value::Bool(b) => i64::from(*b),
        Value::Int8(x) => i64::from(*x),
        Value::UInt8(x) => i64::from(*x),
        Value::Int16(x) => i64::from(*x),
        Value::UInt16(x) => i64::from(*x),
        Value::Int32(x) => i64::from(*x),
        Value::UInt32(x) => i64::from(*x),
        Value::Int64(x) => *x,
        Value::UInt64(x) => *x as i64,
        Value::Raw { bytes, .. } if bytes.len() == 4 => {
            i64::from(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        }
        _ => return None,
    })
}

pub fn values(d: &FieldData) -> Vec<&Value> {
    match d {
        FieldData::Single(v) => vec![v],
        FieldData::Array(vs) => vs.iter().collect(),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UiDropRecord {
    pub name: String,
    pub key: u32,
    pub drop_key: u32,
    pub event_type: Option<EventType>,
    pub scripts: Vec<String>,
    pub avalanche_scripts: Vec<String>,
    pub required_equipment: Vec<u32>,
    pub forced: [u32; 6],
    pub slot_counts: Option<[u32; 5]>,
    pub title: Option<String>,
    pub body: Option<String>,
    pub parameter: i32,
    pub kind: i32,
    pub places: Option<i64>,
    pub goal: Option<i64>,
}

pub fn ui_drop_records(
    cols: &[Collection],
    decoded: &[DecodedCollection],
    index: &HashMap<(u32, u32), usize>,
) -> Result<Vec<UiDropRecord>> {
    let mut out = Vec::new();
    for (i, c) in cols.iter().enumerate().filter(|(_, c)| c.class == CLASS_UI_DROP) {
        let fields = collection::with_inherited(cols, decoded, index, i)?;
        let field = |k: u32| fields.iter().find(|(_, f)| f.key == k).map(|(_, f)| &f.data);
        let drop_key = field(FIELD_UI_DROP_GE_DROP)
            .and_then(|d| {
                values(d).into_iter().find_map(|v| if let Value::Ref(r) = v { Some(r.collection) } else { None })
            })
            .unwrap_or(0);
        let event_type = field(FIELD_UI_DROP_EVENT_TYPE).and_then(|d| match values(d).first() {
            Some(Value::Raw { bytes, .. }) if bytes.len() >= 4 => {
                EventType::from_raw(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
            }
            _ => None,
        });
        let texts = |k: u32| -> Vec<String> {
            field(k).map_or_else(Vec::new, |d| {
                values(d)
                    .into_iter()
                    .filter_map(|v| if let Value::Text(Some(t)) = v { Some(t.clone()) } else { None })
                    .collect()
            })
        };
        let name = texts(FIELD_UI_DROP_NAME).into_iter().next().unwrap_or_else(|| format!("{:08x}", c.key));
        let required_equipment = field(FIELD_UI_DROP_REQ_EQUIPMENT).map_or_else(Vec::new, |d| {
            values(d)
                .into_iter()
                .filter_map(|v| match v {
                    Value::Raw { bytes, .. } if bytes.len() >= 4 => {
                        Some(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                    }
                    _ => None,
                })
                .collect()
        });
        let forced = |k: usize| {
            field(FIELD_UI_DROP_FORCED_LOADOUT).map_or(0, |d| match values(d).get(k).copied() {
                Some(Value::Ref(r)) => r.collection,
                _ => 0,
            })
        };
        let slot_counts = field(FIELD_UI_DROP_SLOT_COUNTS).map(|d| {
            let mut out = [0u32; 5];
            for (o, v) in out.iter_mut().zip(values(d)) {
                *o = match v {
                    Value::UInt8(b) => u32::from(*b).min(4),
                    Value::UInt32(w) => (*w & 0xFF).min(4),
                    _ => 0,
                };
            }
            out
        });
        let word = |k: u32| {
            field(k).and_then(|d| match values(d).first().copied() {
                Some(Value::UInt32(v)) => Some(*v as i32),
                Some(Value::Int32(v)) => Some(*v),
                Some(Value::Raw { bytes, .. }) if bytes.len() >= 4 => {
                    Some(i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                }
                _ => None,
            })
        };
        let text = |k: u32| texts(k).into_iter().next().filter(|t| !t.is_empty());
        out.push(UiDropRecord {
            name,
            key: c.key,
            drop_key,
            event_type,
            scripts: texts(FIELD_UI_DROP_SCRIPTS),
            avalanche_scripts: texts(FIELD_UI_DROP_AVALANCHE_SCRIPTS),
            required_equipment,
            forced: std::array::from_fn(forced),
            slot_counts,
            title: text(FIELD_UI_DROP_HUD_TITLE),
            body: text(FIELD_UI_DROP_HUD_BODY),
            parameter: word(FIELD_UI_DROP_MESSAGE_PARAMETER).unwrap_or(0),
            kind: word(FIELD_UI_DROP_TYPE).unwrap_or(0),
            places: field(FIELD_UI_DROP_PLACES).and_then(|d| values(d).first().and_then(|v| int_of(v))),
            goal: field(FIELD_UI_DROP_TARGETS).and_then(|d| values(d).into_iter().find_map(int_of)),
        });
    }
    Ok(out)
}

pub fn load_ui_drops(big_root: &Path) -> Result<Vec<UiDropRecord>> {
    let dir = big_root.join("bootcache/data/xenon/attribdb");
    let (vlt, bin) = read_pair(&dir.join("attribdb.vlt"))?;
    let (svlt, sbin) = read_pair(&dir.join("attribdb_schema.vlt"))?;
    let schema = Schema::from_vault(&Vault::parse(&svlt, &sbin)?).context("reading the schema")?;
    let vault = Vault::parse(&vlt, &bin).context("parsing attribdb.vlt")?;
    let cols = collection::collections(&vault)?;
    let decoded = cols.iter().map(|c| c.decode(&vault, &schema)).collect::<ssx_formats::Result<Vec<_>>>()?;
    ui_drop_records(&cols, &decoded, &collection::index(&cols))
}

#[cfg(test)]
mod tests {
    use super::{EventType, shows_explore_ghosts};

    #[test]
    fn each_event_type_has_its_kind_word() {
        let words: Vec<&str> = (0..9).map(|v| EventType::from_raw(v).expect("a type").kind_word()).collect();
        assert_eq!(words, ["trick", "trick", "trick", "survive", "survive", "race", "race", "race", "freeride"]);
        assert_eq!(EventType::from_raw(9), None);
    }

    #[test]
    fn only_ghost_events_race_explore_ghosts() {
        let ghosts: Vec<u32> =
            (0..9).filter(|&v| EventType::from_raw(v).expect("a type").has_explore_ghosts()).collect();
        assert_eq!(ghosts, [EventType::TrickGhosts as u32, EventType::RaceGhosts as u32]);
    }

    #[test]
    fn explore_ghosts_show_only_in_ghost_events_without_bots() {
        assert!(shows_explore_ghosts(Some(EventType::RaceGhosts), false));
        assert!(shows_explore_ghosts(Some(EventType::TrickGhosts), false));
        assert!(!shows_explore_ghosts(Some(EventType::RaceGhosts), true));
        assert!(!shows_explore_ghosts(Some(EventType::SurviveToBottom), false));
        assert!(!shows_explore_ghosts(Some(EventType::SurviveToDeath), false));
        assert!(!shows_explore_ghosts(None, false));
    }
}
