use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use glam::Vec3;
use ssx_formats::attrib::collection::{self, FieldData};
use ssx_formats::attrib::{Schema, Value, Vault, fnv1};

use ssx_assets::career::random::GameRandom;

use crate::event::Rival;

pub const CLASS_BOT_CATCHUP: u32 = fnv1(b"bot_catchup");
pub const FIELD_CATCHUP_CURVE: u32 = 0xFC3D_2D7F;
pub const CATCHUP_NONE: u32 = fnv1(b"none");
pub const FIELD_UI_DROP_RIVAL_CHARACTERS: u32 = 0xC211_FF58;
pub const CLASS_CHARACTER: u32 = fnv1(b"character");
pub const CLASS_UI_DROP: u32 = fnv1(b"ui_drop");
pub const BOT_TINT: u32 = 0xFF05_B8F3;
pub const GENERIC_CHARACTER: u32 = fnv1(b"griff");
pub const GENERIC_TINT: u32 = 0xFF50_4E0C;
pub const CATCHUP_CLAMP_HEIGHT: f32 = 500.0;
pub const GHOST_COLLISION_DISTANCE: f32 = 0.75;

#[derive(Clone, Debug, PartialEq)]
pub struct CatchUpCurve {
    pub points: Vec<(f32, f32)>,
}

impl CatchUpCurve {
    pub fn from_raw(b: &[u8]) -> Option<Self> {
        let f = |o: usize| b.get(o..o + 4).map(|w| f32::from_be_bytes([w[0], w[1], w[2], w[3]]));
        let n = u32::from_be_bytes(b.get(0x10..0x14)?.try_into().ok()?) as usize;
        if n == 0 || n > 8 {
            return None;
        }
        let points = (0..n).map(|k| Some((f(0x14 + 4 * k)?, f(0x34 + 4 * k)?))).collect::<Option<Vec<_>>>()?;
        Some(Self { points })
    }

    pub fn eval(&self, x: f32) -> f32 {
        let p = &self.points;
        let n = p.len();
        if x <= p[0].0 {
            return p[0].1;
        }
        let Some(i) = p.iter().position(|q| x < q.0) else { return p[n - 1].1 };
        let (pm, p0, p1, p2) = (p[i.saturating_sub(2)], p[i - 1], p[i], p[(i + 1).min(n - 1)]);
        let t = (x - p0.0) / (p1.0 - p0.0);
        let (t2, t3) = (t * t, t * t * t);
        let m0 = 0.5 * (p1.1 - pm.1);
        let m1 = 0.5 * (p2.1 - p0.1);
        (2.0 * t3 - 3.0 * t2 + 1.0) * p0.1 + (t3 - 2.0 * t2 + t) * m0 + (3.0 * t2 - 2.0 * t3) * p1.1 + (t3 - t2) * m1
    }
}

pub fn catch_up_factor(curve: Option<&CatchUpCurve>, lead: f32, height_left: f32, finished: bool) -> f32 {
    let f = match curve {
        Some(c) if !finished => c.eval(lead),
        _ => 1.0,
    };
    if height_left < CATCHUP_CLAMP_HEIGHT { f.clamp(0.0, 1.0) } else { f }
}

pub fn descent_and_height_left(start_y: f32, end_y: f32, y: f32) -> (f32, f32) {
    let range = start_y - end_y;
    ((start_y - y).max(0.0).min(range), (y - end_y).max(0.0).min(range))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BotRequest {
    pub ghost_id: i32,
    pub start_slot: i32,
    pub character: u32,
    pub catchup: u32,
    pub racer_type: u32,
    pub collision: bool,
    pub tint: u32,
    pub score: u32,
}

pub fn bot_requests(
    rival_characters: &[u32],
    rows: &[Rival],
    known: impl Fn(u32) -> bool,
    mut pick: impl FnMut(usize) -> usize,
) -> Vec<BotRequest> {
    let mut chosen = Vec::new();
    let mut take = |cands: Vec<&Rival>, chosen: &mut Vec<Rival>| match cands.len() {
        0 => {}
        1 => chosen.push(*cands[0]),
        n => chosen.push(*cands[pick(n).min(n - 1)]),
    };
    take(rows.iter().filter(|r| !known(r.character)).collect(), &mut chosen);
    for &c in rival_characters {
        take(rows.iter().filter(|r| r.character == c).collect(), &mut chosen);
    }
    chosen
        .into_iter()
        .map(|r| {
            let generic = !known(r.character);
            BotRequest {
                ghost_id: r.ghost_id,
                start_slot: r.starting_idx,
                character: if generic { GENERIC_CHARACTER } else { r.character },
                catchup: r.catchup,
                racer_type: 1,
                collision: true,
                tint: if generic { GENERIC_TINT } else { BOT_TINT },
                score: r.score,
            }
        })
        .collect()
}

pub const EXPLORE_RACER_TYPE: u32 = 2;

pub fn explore_requests(def: &crate::event::EventDef) -> Vec<BotRequest> {
    def.ghost_board()
        .into_iter()
        .map(|g| {
            let row = g.row.and_then(|i| def.rivals.get(i));
            BotRequest {
                ghost_id: row.map_or(-1, |r| r.ghost_id),
                start_slot: row.map_or(0, |r| r.starting_idx),
                character: g.character,
                catchup: row.map_or(0, |r| r.catchup),
                racer_type: EXPLORE_RACER_TYPE,
                collision: false,
                tint: BOT_TINT,
                score: row.map_or(0, |r| r.score),
            }
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BotClock {
    pub seconds: f32,
}

impl BotClock {
    pub fn advance(&mut self, factor: f32, time_scale_override: Option<f32>, dt_s: f32) {
        self.seconds += factor * time_scale_override.unwrap_or(1.0) * dt_s;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AvoidInputs {
    pub position: Vec3,
    pub velocity: Vec3,
    pub player_location: Vec3,
    pub player_velocity: Vec3,
}

pub fn playback_time_scale(scale: f32, avoid: Option<AvoidInputs>) -> (f32, bool) {
    let (target, rate) = match avoid {
        Some(a) => {
            let d = a.position - a.player_location;
            let w = a.velocity - a.player_velocity;
            let (dn, wn, vn) = (d.normalize_or_zero(), w.normalize_or_zero(), a.player_velocity.normalize_or_zero());
            let delta = if dn.dot(wn) < 0.0 { if dn.dot(vn) > 0.0 { 2.0 } else { -1.0 } } else { 0.0 };
            (1.0 + delta, 1.0 / (1.0 + 0.5 * d.length()))
        }
        None => (1.0, 0.1),
    };
    let s = scale + (target - scale).clamp(-rate, rate);
    (s, (s - 1.0).abs() <= 1.192_092_9e-7)
}

// TODO(guess): the `Distance Filter`'s `Comparison` 2 read as "closer than" (its inputs unresolved) and the `GameState Compare` filter as passed; settle: `DistanceQueryFilterAsset`'s and `ValueCompareQueryFilterAsset`'s evaluation (the query runtime's kernels).
pub fn ghost_query(enabled: bool, bot_position: Vec3, bot_velocity: Vec3, player_position: Vec3) -> Option<Vec3> {
    if enabled {
        crate::guess_hit!();
    }
    (enabled && bot_position.distance(player_position) < GHOST_COLLISION_DISTANCE).then_some(bot_velocity)
}

fn read_pair(vlt: &Path) -> Result<(Vec<u8>, Vec<u8>)> {
    let bin = vlt.with_extension("bin");
    Ok((
        std::fs::read(vlt).with_context(|| format!("reading {}", vlt.display()))?,
        std::fs::read(&bin).with_context(|| format!("reading {}", bin.display()))?,
    ))
}

fn values(d: &FieldData) -> Vec<&Value> {
    match d {
        FieldData::Single(v) => vec![v],
        FieldData::Array(vs) => vs.iter().collect(),
    }
}

#[derive(Clone, Debug, Default)]
pub struct BotData {
    pub curves: HashMap<u32, CatchUpCurve>,
    pub characters: Vec<u32>,
    pub rival_characters: HashMap<u32, Vec<u32>>,
}

impl BotData {
    pub fn load(big_root: &Path) -> Result<Self> {
        let dir = big_root.join("bootcache/data/xenon/attribdb");
        let (vlt, bin) = read_pair(&dir.join("attribdb.vlt"))?;
        let (svlt, sbin) = read_pair(&dir.join("attribdb_schema.vlt"))?;
        let schema = Schema::from_vault(&Vault::parse(&svlt, &sbin)?).context("reading the schema")?;
        let vault = Vault::parse(&vlt, &bin).context("parsing attribdb.vlt")?;
        let cols = collection::collections(&vault)?;
        let mut decoded = cols.iter().map(|c| c.decode(&vault, &schema)).collect::<ssx_formats::Result<Vec<_>>>()?;
        ssx_assets::content::data::events::route_ui_drops(big_root, &cols, &mut decoded);
        let index = collection::index(&cols);
        let mut out = Self::default();
        for (i, c) in cols.iter().enumerate() {
            if c.class == CLASS_CHARACTER {
                out.characters.push(c.key);
                continue;
            }
            if c.class != CLASS_BOT_CATCHUP && c.class != CLASS_UI_DROP {
                continue;
            }
            let fields = collection::with_inherited(&cols, &decoded, &index, i)?;
            let field = |k: u32| fields.iter().find(|(_, f)| f.key == k).map(|(_, f)| &f.data);
            if c.class == CLASS_BOT_CATCHUP {
                if let Some(Value::Raw { bytes, .. }) =
                    field(FIELD_CATCHUP_CURVE).and_then(|d| values(d).first().copied())
                    && let Some(curve) = CatchUpCurve::from_raw(bytes)
                {
                    out.curves.insert(c.key, curve);
                }
            } else if let Some(d) = field(FIELD_UI_DROP_RIVAL_CHARACTERS) {
                let chars: Vec<u32> = values(d)
                    .into_iter()
                    .filter_map(|v| if let Value::Ref(r) = v { Some(r.collection) } else { None })
                    .filter(|&k| k != 0)
                    .collect();
                out.rival_characters.insert(c.key, chars);
            }
        }
        if let Ok((gv, gb)) = read_pair(&dir.join("ge_attribdb.vlt")) {
            let gvault = Vault::parse(&gv, &gb).context("parsing ge_attribdb.vlt")?;
            out.characters
                .extend(collection::collections(&gvault)?.iter().filter(|c| c.class == CLASS_CHARACTER).map(|c| c.key));
        }
        Ok(out)
    }
}

pub const CHARACTER_NAMES: [&str; 12] =
    ["alex", "eddie", "elise", "griff", "kaori", "mac", "moby", "psymon", "tane", "travis", "tyler", "zoe"];

pub fn character_name(key: u32) -> &'static str {
    CHARACTER_NAMES.iter().copied().find(|n| fnv1(n.as_bytes()) == key).unwrap_or("griff")
}

pub const DEFAULT_BOT_SEED: u32 = 1;

#[derive(Clone, Debug)]
pub struct Bot {
    pub request: BotRequest,
    pub replay: ssx_assets::ghosts::GhostReplay,
    pub curve: Option<CatchUpCurve>,
    pub clock: BotClock,
    pub position: Vec3,
    pub velocity: Vec3,
    pub factor: f32,
    pub lead: f32,
    pub finish_ms: Option<u32>,
    pub removed: bool,
    pub trigger_wipeout: bool,
    pub player_is_grinding: bool,
    pub in_air: bool,
    pub avoid_grind_collision: bool,
    pub player_location: Vec3,
    pub player_velocity: Vec3,
    pub contacts: u32,
    pub wipeout: Option<wipeout::Wipeout>,
}

pub const GHOST_GRINDING_DISTANCE: f32 = 20.0;

#[derive(Clone, Debug, Default)]
pub struct BotRace {
    pub bots: Vec<Bot>,
    pub wipeout: Option<std::sync::Arc<wipeout::WipeoutFlow>>,
    pub course: Option<crate::event::Course>,
    pub running: bool,
    pub forced_wipeout: Option<usize>,
    pub scores: Vec<u32>,
    pub live: Vec<live::LiveRacer>,
    pub live_slots: live::LiveSlots,
}

impl BotRace {
    pub fn load(
        big_root: &Path,
        def: &crate::event::EventDef,
        data: &BotData,
        rng: &mut dyn GameRandom,
        course: Option<crate::event::Course>,
    ) -> (Self, Vec<String>) {
        let chars = data.rival_characters.get(&def.key).cloned().unwrap_or_default();
        let known = |c: u32| data.characters.contains(&c);
        let reqs = bot_requests(&chars, &def.rivals, known, |n| rng.below(n as u32) as usize);
        let scores = reqs.iter().map(|r| r.score).collect();
        Self::load_requests(big_root, reqs, scores, data, course)
    }

    pub fn for_event(
        big_root: &Path,
        def: &crate::event::EventDef,
        data: &BotData,
        rng: &mut dyn GameRandom,
        course: Option<crate::event::Course>,
    ) -> Option<(Self, Vec<String>)> {
        use crate::event::EventType;
        match def.event_type? {
            EventType::RaceBots | EventType::TrickBots => Some(Self::load(big_root, def, data, rng, course)),
            EventType::RaceGhosts | EventType::TrickGhosts => {
                Some(Self::load_requests(big_root, explore_requests(def), Vec::new(), data, course))
            }
            _ => None,
        }
    }

    pub fn load_requests(
        big_root: &Path,
        reqs: Vec<BotRequest>,
        scores: Vec<u32>,
        data: &BotData,
        course: Option<crate::event::Course>,
    ) -> (Self, Vec<String>) {
        let (mut bots, mut problems) = (Vec::new(), Vec::new());
        let dir = big_root.join(ssx_assets::rig_anim::ANIM_DIR);
        let flow = ssx_assets::anim_db::AssetDb::load(&dir, &[ssx_assets::rig_anim::RIG_BUNDLE])
            .and_then(|db| wipeout::WipeoutFlow::load(&db));
        let flow = match flow {
            Ok(f) => Some(std::sync::Arc::new(f)),
            Err(e) => {
                problems.push(format!("sf_ghost_wipeout: {e:#}"));
                None
            }
        };
        for r in reqs {
            match ssx_assets::ghosts::GhostReplay::load(big_root, r.ghost_id as u32) {
                Ok(replay) => {
                    let p0 = Vec3::from_array(replay.position(0.0));
                    bots.push(Bot {
                        request: r,
                        curve: data.curves.get(&r.catchup).cloned(),
                        replay,
                        clock: BotClock::default(),
                        position: p0,
                        velocity: Vec3::ZERO,
                        factor: 1.0,
                        lead: 0.0,
                        finish_ms: None,
                        removed: false,
                        trigger_wipeout: false,
                        in_air: false,
                        player_is_grinding: false,
                        avoid_grind_collision: false,
                        player_location: Vec3::ZERO,
                        player_velocity: Vec3::ZERO,
                        contacts: 0,
                        wipeout: flow.as_deref().map(wipeout::Wipeout::new),
                    });
                }
                Err(e) => problems.push(format!("bot ghost {}: {e:#}", r.ghost_id)),
            }
        }
        let race =
            Self { bots, wipeout: flow, course, running: false, forced_wipeout: None, scores, ..Self::default() };
        (race, problems)
    }

    pub fn step(&mut self, s: &mut crate::rider::RiderState, race_start: bool, dt_s: f32) {
        if race_start {
            self.running = true;
            for b in &mut self.bots {
                b.clock = BotClock::default();
            }
        }
        if !self.running {
            return;
        }
        let course = self.course.unwrap_or(crate::event::Course { start_y: 0.0, end_y: -1.0 });
        let (player_descent, _) = descent_and_height_left(course.start_y, course.end_y, s.traj.y);
        let grinding = s.grind.is_grinding();
        let mut contact = None;
        let forced = self.forced_wipeout.take();
        for (k, b) in self.bots.iter_mut().enumerate().filter(|(_, b)| !b.removed) {
            let (descent, height_left) = descent_and_height_left(course.start_y, course.end_y, b.position.y);
            b.lead = descent - player_descent;
            b.factor = catch_up_factor(b.curve.as_ref(), b.lead, height_left, b.finish_ms.is_some());
            let scale = b.wipeout.as_ref().and_then(wipeout::Wipeout::time_scale_override);
            b.clock.advance(b.factor, scale, dt_s);
            if b.replay.finished(b.clock.seconds) {
                b.removed = true;
                continue;
            }
            let pose = b.replay.pose(b.clock.seconds);
            b.position = Vec3::from_array(b.replay.position(b.clock.seconds));
            b.velocity = Vec3::from_array(pose.velocity);
            if let Some(w) = b.wipeout.as_mut().filter(|_| b.replay.times.len() >= 2) {
                // TODO(guess): the ghost's velocity operand (rig set +0x58 entry +0x30) is taken as the replayed `VEC_Velocity`: the avoid branch of `RewindPlayback_GhostPose` 0x82C652B0 (0x82C65548…0x82C65930) takes `w` = that − `VEC_PlayerVelocity` beside `d` = rig set +0x44 entry +0x20 (the trajectory position) − `VEC_PlayerLocation`; settle: the rig set's writer.
                crate::guess_hit!();
                let avoid = (b.player_is_grinding && b.avoid_grind_collision).then_some(AvoidInputs {
                    position: b.position,
                    velocity: b.velocity,
                    player_location: b.player_location,
                    player_velocity: b.player_velocity,
                });
                (w.time_scale, w.override_time_scale) = playback_time_scale(w.time_scale, avoid);
            }
            b.avoid_grind_collision = b.position.distance(s.traj) < GHOST_GRINDING_DISTANCE;
            b.player_is_grinding = grinding && b.avoid_grind_collision;
            if b.avoid_grind_collision {
                (b.player_location, b.player_velocity) = (s.traj_cached, s.traj_velocity);
            }
            b.trigger_wipeout = false;
            if let Some(v) = ghost_query(b.request.collision, b.position, b.velocity, s.traj) {
                b.trigger_wipeout = true;
                b.contacts += 1;
                contact = Some(v);
            }
            if forced == Some(k) {
                b.trigger_wipeout = true;
            }
            let in_air = pose.flags & ssx_formats::ghost::FLAG_IN_AIR != 0;
            b.in_air = in_air;
            if let (Some(w), Some(flow)) = (b.wipeout.as_mut(), self.wipeout.as_deref()) {
                w.update(flow, b.trigger_wipeout, b.player_is_grinding, in_air, dt_s * 60.0);
            }
        }
        if contact.is_some() {
            s.ghost_impact = contact;
        }
    }

    pub fn reset(&mut self) {
        self.running = false;
        for b in &mut self.bots {
            b.clock = BotClock::default();
            b.position = Vec3::from_array(b.replay.position(0.0));
            (b.velocity, b.factor, b.lead, b.finish_ms, b.removed) = (Vec3::ZERO, 1.0, 0.0, None, false);
            (b.trigger_wipeout, b.player_is_grinding, b.contacts) = (false, false, 0);
            (b.avoid_grind_collision, b.player_location, b.player_velocity) = (false, Vec3::ZERO, Vec3::ZERO);
            b.wipeout = self.wipeout.as_deref().map(wipeout::Wipeout::new);
        }
    }

    pub fn racers(&self, w: &crate::rider::World) -> [Option<crate::event::RacerInput>; crate::event::MAX_RACERS] {
        let mut out = [None; crate::event::MAX_RACERS];
        let set = &w.triggers;
        let touches_end = |p: Vec3| {
            set.touching(p, crate::triggers::PLAYER_TOUCH_RADIUS, crate::triggers::IG_GHOST)
                .any(|i| set.volumes[i].name == "end")
        };
        for (k, id) in self.live_slots.occupants() {
            if let Some(r) = self.live.iter().find(|r| r.id == id) {
                out[k] = Some(crate::event::RacerInput {
                    y: r.position.y,
                    touching_end: touches_end(r.position),
                    points: 0.0,
                    trigger_wipeout: false,
                    in_air: r.in_air,
                });
            }
        }
        for (k, b) in self.bots.iter().take(crate::event::MAX_RACERS).enumerate() {
            let touching_end = !b.removed && touches_end(b.position);
            out[k] = Some(crate::event::RacerInput {
                y: b.position.y,
                touching_end,
                points: b.replay.fed_score(b.clock.seconds),
                trigger_wipeout: b.trigger_wipeout,
                in_air: b.in_air,
            });
        }
        out
    }
}

pub mod live;
pub mod wipeout;

#[cfg(test)]
mod tests;
