use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use glam::{Quat, Vec3};
use ssx_assets::career::geotags::{RIDE_GEOTAGS, geotag_key, pickup_value};
use ssx_assets::career::random::GameRandom;
use ssx_assets::track_objects::Placement;
use ssx_assets::track_objects::geotags::{GeotagController, offline_tier};
use ssx_assets::vault_db::{fnv1, with_track_db};
use ssx_formats::csvdb::Geotag;
use ssx_lua::game::controllers::{Controller, ControllerWorld, Phase, Track, TrackCall};
use ssx_lua::game::objectives::Name;

use crate::triggers::{IG_PLAYER, PLAYER_TOUCH_RADIUS, TriggerVolume, TriggerVolumeRecord};

pub const MIN_SPACING_M: i32 = 100;

pub const DEFAULT_GEOTAG_SEED: u32 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct GeotagRecord {
    pub key: u64,
    pub row: Geotag,
    pub value: i64,
    pub tier: usize,
}

pub fn place(
    rows: &[Geotag],
    track_key: u32,
    collected: impl Fn(u64) -> bool,
    rng: &mut dyn GameRandom,
) -> Vec<GeotagRecord> {
    let mut list = records(rows, track_key, collected);
    thin(&mut list, RIDE_GEOTAGS, MIN_SPACING_M, rng);
    list
}

pub fn records(rows: &[Geotag], track_key: u32, collected: impl Fn(u64) -> bool) -> Vec<GeotagRecord> {
    rows.iter()
        .filter_map(|g| {
            let key = geotag_key(track_key, g.id);
            (!collected(key)).then(|| GeotagRecord {
                key,
                row: *g,
                value: pickup_value(g.level, g.quality),
                tier: offline_tier(g.level),
            })
        })
        .collect()
}

pub fn thin(list: &mut Vec<GeotagRecord>, max: usize, spacing: i32, rng: &mut dyn GameRandom) {
    let d2 = (spacing.wrapping_mul(spacing) as u32) as f32;
    let mut i = 0;
    while i < list.len() {
        let a = Vec3::from_array(list[i].row.position);
        let mut j = i;
        while j < list.len() {
            let b = Vec3::from_array(list[j].row.position);
            let d = a - b;
            if list[j].key != list[i].key && d2 >= d.x * d.x + d.y * d.y + d.z * d.z {
                list.remove(j);
            } else {
                j += 1;
            }
        }
        i += 1;
    }
    let n0 = list.len() as u32;
    let mut i = 0;
    while i < list.len() && list.len() > max {
        if rng.below(n0) as usize > max {
            list.remove(i);
        } else {
            i += 1;
        }
    }
    while list.len() > max {
        list.remove(0);
    }
}

#[derive(Clone, Debug)]
pub struct GeotagData {
    pub track: String,
    pub track_key: u32,
    pub rows: Vec<Geotag>,
    pub controller: GeotagController,
    pub volumes: Vec<TriggerVolumeRecord>,
    pub disc_total: usize,
}

impl GeotagData {
    pub fn load(big_root: &Path, track: &str) -> Result<Option<Self>> {
        use ssx_assets::track_objects::geotags as g;
        let Some(rows) = g::load_rows(big_root, track)? else { return Ok(None) };
        let assets = g::load(big_root, track)?;
        let volumes = with_track_db(big_root, track, |db| {
            assets
                .objects
                .iter()
                .map(|o| crate::spawn::trigger_volume_record(db, o.trigger_volume.0, o.trigger_volume.1))
                .collect::<Result<Vec<_>>>()
        })?;
        for (v, o) in volumes.iter().zip(&assets.objects) {
            anyhow::ensure!(
                v.name == assets.controller.collect_volume,
                "{}: volume {:?}, the controller tests {:?}",
                o.game_object,
                v.name,
                assets.controller.collect_volume
            );
        }
        Ok(Some(GeotagData {
            track: track.to_string(),
            track_key: fnv1(&format!("{track}_track")),
            rows,
            controller: assets.controller,
            volumes,
            disc_total: g::disc_total(big_root).context("the geotag manifest")?,
        }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GeotagPhase {
    Idle,
    Collecting,
    PostCollected,
}

#[derive(Clone, Debug, PartialEq)]
enum GeotagInst {
    Bounce(f32),
    Move { start: Vec3, remaining: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeotagEntity {
    pub record: GeotagRecord,
    pub position: Vec3,
    pub rotation: Quat,
    pub controller: Controller,
    inst: std::collections::BTreeMap<u64, GeotagInst>,
    pub phase: GeotagPhase,
    pub collected: bool,
    // TODO(guess): +0x50 (render interface 0x992F2472 slot 0x14 / 0x18) is read as "drawn" from its writers alone (spawn, pickup, the manager's ¬BLN_DisableWorldRender, 0x82448D38); settle: the renderer's read of the render instance's +0x50 (the class of vtable 0x8210CEF4).
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GeotagEvent {
    Touched { entity: usize },
    Collected { entity: usize, key: u64, id: u32, value: i64, tier: usize, position: Vec3, rotation: Quat },
}

#[derive(Clone, Debug)]
pub struct GeotagRun {
    pub data: Arc<GeotagData>,
    pub entities: Vec<GeotagEntity>,
    pub result_list: Vec<i64>,
    pub shown: bool,
    pub events: Vec<GeotagEvent>,
    started: bool,
}

impl GeotagRun {
    pub fn new(data: Arc<GeotagData>, collected: impl Fn(u64) -> bool, rng: &mut dyn GameRandom) -> Self {
        let records = place(&data.rows, data.track_key, collected, rng);
        let entities = records
            .into_iter()
            .take(RIDE_GEOTAGS)
            .map(|record| GeotagEntity {
                position: Vec3::from_array(record.row.position),
                record,
                rotation: Quat::IDENTITY,
                controller: Controller::new(data.controller.sets.clone(), data.controller.set),
                inst: Default::default(),
                phase: GeotagPhase::Idle,
                collected: false,
                visible: false,
            })
            .collect();
        GeotagRun { data, entities, result_list: Vec::new(), shown: false, events: Vec::new(), started: false }
    }

    pub fn credits(&self) -> i64 {
        self.result_list.iter().sum()
    }

    pub fn volume(&self, e: &GeotagEntity) -> TriggerVolume {
        let pl = Placement {
            scale: [1.0; 3],
            rotation: e.rotation.to_array(),
            position: e.position.to_array(),
            unk: [1.0, 1.0, 1.0, 0.0, 0.0, 0.0],
        };
        TriggerVolume::place(&self.data.volumes[e.record.tier], &pl)
    }

    pub fn update(&mut self, dt_s: f32, player: Vec3, world_hidden: bool) {
        self.events.clear();
        let show = !world_hidden;
        if show != self.shown {
            for e in &mut self.entities {
                e.visible = show;
            }
            self.shown = show;
        }
        let first = !std::mem::replace(&mut self.started, true);
        for i in 0..self.entities.len() {
            let mut e = self.entities[i].clone();
            let mut controller = std::mem::replace(&mut e.controller, Controller::new(Default::default(), 0));
            let before = e.phase;
            {
                let mut w = GeotagWorld { run: self, e: &mut e, i, player };
                if first {
                    let tier = w.e.record.tier;
                    let default = w.run.data.controller.default_states[tier].as_deref().map(Name::new);
                    controller.set_state(default.as_ref(), &mut w);
                }
                controller.update(dt_s, &mut w);
            }
            let states = controller.states();
            e.phase = if states.iter().any(|s| s == "PostCollected") {
                GeotagPhase::PostCollected
            } else if states.iter().any(|s| s == "Collecting") {
                GeotagPhase::Collecting
            } else {
                GeotagPhase::Idle
            };
            if before == GeotagPhase::Idle && e.phase != GeotagPhase::Idle {
                let at = self
                    .events
                    .iter()
                    .position(|ev| matches!(ev, GeotagEvent::Collected { entity, .. } if *entity == i));
                self.events.insert(at.unwrap_or(self.events.len()), GeotagEvent::Touched { entity: i });
            }
            e.controller = controller;
            self.entities[i] = e;
        }
    }
}

struct GeotagWorld<'a> {
    run: &'a mut GeotagRun,
    e: &'a mut GeotagEntity,
    i: usize,
    player: Vec3,
}

impl ControllerWorld for GeotagWorld<'_> {
    fn trigger_volume(&mut self, volume: &str, entity: &str, client: &str) -> Option<bool> {
        if entity != crate::controllers::PLAYER || client != "Touch" {
            return None;
        }
        let v = self.run.volume(self.e);
        Some(
            (volume.is_empty() || v.name == volume)
                && v.interacts & IG_PLAYER != 0
                && v.overlaps_sphere(self.player, PLAYER_TOUCH_RADIUS),
        )
    }
    fn track(&mut self, c: TrackCall<'_>, phase: Phase) {
        let e = &mut *self.e;
        match (c.kind, phase) {
            (Track::TransformAnimation { rotate, bounce }, Phase::Update(dt)) => {
                if let Some(r) = rotate {
                    let half = r * dt * 0.5;
                    let q = Quat::from_xyzw(0.0, half.sin(), 0.0, half.cos());
                    e.rotation = (q * e.rotation).normalize();
                }
                if let Some((amplitude, speed)) = bounce {
                    let GeotagInst::Bounce(phi) = e.inst.entry(c.inst).or_insert(GeotagInst::Bounce(0.0)) else {
                        return;
                    };
                    let before = phi.sin().abs();
                    *phi += speed * dt;
                    e.position.y += amplitude * (phi.sin().abs() - before);
                }
            }
            (Track::MoveToEntity { time, .. }, Phase::Begin) => {
                e.inst.insert(c.inst, GeotagInst::Move { start: e.position, remaining: *time });
            }
            (Track::MoveToEntity { time, .. }, Phase::Update(dt)) => {
                if let Some(GeotagInst::Move { start, remaining }) = e.inst.get_mut(&c.inst) {
                    *remaining -= dt;
                    let p = self.player;
                    e.position = if *remaining > 0.0 { p + (*start - p) * (*remaining / time) } else { p };
                }
            }
            (Track::MoveToEntity { .. } | Track::TransformAnimation { .. }, Phase::End) => {
                e.inst.remove(&c.inst);
            }
            (Track::GeoCache, Phase::Begin) => {
                (e.visible, e.collected) = (false, true);
                let r = &e.record;
                self.run.result_list.push(r.value);
                self.run.events.push(GeotagEvent::Collected {
                    entity: self.i,
                    key: r.key,
                    id: r.row.id,
                    value: r.value,
                    tier: r.tier,
                    position: e.position,
                    rotation: e.rotation,
                });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssx_assets::career::random::Mt19937;

    const SET: &str = r#"
local s = CreateStateSet( "GeoCache" )
local st = s:AddState( "Active" )
st:AddUpdateTrack( SubStateTrack( "Passive" ) )
st:AddUpdateTrack( SubStateTrack( "CollectionIdle" ) )
st = s:AddState( "Passive" )
local t = TransformAnimationTrack()
t:Rotate( 300.0 )
t:Bounce( 1.0, 4.0 )
st:AddUpdateTrack( t )
st = s:AddState( "CollectionIdle" )
local tr = st:AddTransition( "Collecting" )
tr:AddCondition( TriggerVolumeCondition( "world_geotag_collect_group", "Player1", "Touch" ) )
st = s:AddState( "Collecting" )
st:AddUpdateTrack( MoveToEntityTrack( "Player1", 0.25 ) )
st:AddUpdateTrack( SubStateTrack( "Collected" ), 0.25 )
tr = st:AddTransition( "PostCollected" )
tr:AddCondition( StateTimeCondition( 4.0 ) )
st = s:AddState( "Collected" )
st:AddEnterTrack( GeoCacheTrack( "x" ) )
st = s:AddState( "PostCollected" )
"#;

    fn row(id: u32, x: f32, level: u32) -> Geotag {
        Geotag { id, quality: 1, level, position: [x, 0.0, 0.0] }
    }

    fn data(rows: Vec<Geotag>) -> Arc<GeotagData> {
        let volume = TriggerVolumeRecord {
            name: "world_geotag_collect_group".into(),
            type_text: "collect".into(),
            shape: crate::triggers::SHAPE_BOX,
            scale: Vec3::splat(6.0),
            position: Vec3::new(0.0, 1.5, 0.0),
            orientation: Quat::IDENTITY,
        };
        Arc::new(GeotagData {
            track: "test".into(),
            track_key: 7,
            rows,
            controller: ssx_assets::track_objects::geotags::geotag_controller(crate::controllers::script_sets(
                "c.lua", SET,
            ))
            .expect("the test set"),
            volumes: vec![volume; 3],
            disc_total: 599,
        })
    }

    #[test]
    fn spacing_keeps_the_first_of_close_pairs_and_the_walk_keeps_25_in_order() {
        let rows = [row(1, 0.0, 5), row(2, 50.0, 6), row(3, 150.0, 8), row(4, 250.0, 1), row(5, 400.0, 2)];
        let mut rng = Mt19937::seeded(DEFAULT_GEOTAG_SEED);
        let kept = place(&rows, 7, |k| k == geotag_key(7, 5), &mut rng);
        assert_eq!(kept.iter().map(|r| r.row.id).collect::<Vec<_>>(), [1, 3]);
        assert_eq!((kept[0].tier, kept[1].tier, kept[1].value), (0, 2, 51_200));
        let many: Vec<Geotag> = (0..40).map(|i| row(i, i as f32 * 200.0, 4)).collect();
        let kept = place(&many, 7, |_| false, &mut rng);
        assert_eq!(kept.len(), RIDE_GEOTAGS);
        assert!(kept.windows(2).all(|w| w[0].row.id < w[1].row.id), "file order is kept");
    }

    #[test]
    fn a_geotag_spins_bounces_flies_to_the_rider_and_pays_once() {
        let mut rng = Mt19937::seeded(1);
        let mut run = GeotagRun::new(data(vec![row(9, 0.0, 6)]), |_| false, &mut rng);
        let dt = 1.0 / 30.0;
        let far = Vec3::new(0.0, 0.0, 50.0);
        for _ in 0..30 {
            run.update(dt, far, false);
        }
        let e = &run.entities[0];
        assert!(e.visible && e.phase == GeotagPhase::Idle);
        let (axis, angle) = e.rotation.to_axis_angle();
        assert!((axis.y.abs() - 1.0).abs() < 1e-4 && ((angle - 300f32.to_radians()).abs() < 1e-3));
        assert!((e.position.y - 4f32.sin().abs()).abs() < 1e-4, "y {}", e.position.y);
        let rider = Vec3::new(2.0, 1.0, 2.5);
        run.update(dt, rider, false);
        assert_eq!(run.events, [GeotagEvent::Touched { entity: 0 }]);
        let mut collected = Vec::new();
        for k in 0..130 {
            run.update(dt, rider, false);
            if run.events.iter().any(|ev| matches!(ev, GeotagEvent::Collected { .. })) {
                collected.push(k);
            }
        }
        assert_eq!(collected, [6]);
        assert_eq!(run.credits(), pickup_value(6, 1));
        assert_eq!(run.entities[0].phase, GeotagPhase::PostCollected);
        assert!(!run.entities[0].visible);
    }
}
