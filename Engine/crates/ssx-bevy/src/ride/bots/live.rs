use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, TryRecvError};

use anyhow::Result;
use bevy::prelude::{Commands, Res, ResMut, info, warn};

use ssx_assets::content::characters::budget::RiderLooks;
use ssx_assets::content::characters::dress::LookChoice;
use ssx_content::{ContentId, Kind};

use super::{BotVisual, Bots, FrameFacts, LoadedBot, RacerDraw, RacerSources, RacerSpawner, load_racer_as};
use crate::rider::anim::{JointRecord, world_records};

const PLAYER_IDS: u64 = 0x4000_0001;

// PORT(choice): the explore racers' request colour 0xFF05B8F3 (`[0x837E6FA8]`, `GameMode_BuildExploreGhostRequests`
// 0x82480840), as the game gave its replayed online racers a request colour; a port feature: the original has no
// real-time racers, so no colour of a live player's name to decode.
const NAME_COLOUR: u32 = 0xFF05_B8F3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveLook {
    pub name: String,
    pub character: u32,
    pub gear: u16,
    pub hologram: bool,
}

struct LiveVisual {
    id: u16,
    look: LiveLook,
    visual: BotVisual,
    trajectory: Option<JointRecord>,
    spray: Option<(JointRecord, ssx_assets::ghosts::GhostPose)>,
}

type Loading = (LiveLook, Mutex<Receiver<Result<LoadedBot>>>);

// PORT(choice): two loader threads at most, counted until each ends (also those whose racer left meanwhile), bound the
// threads and the memory a join flood can start; a port feature: the original has no live racers.
const MAX_LOADS: usize = 2;

struct LoaderCount(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl Drop for LoaderCount {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }
}

#[derive(Clone, Copy)]
pub(super) struct LiveFrame {
    pub facts: FrameFacts,
    pub dt_ticks: f32,
    pub racers: usize,
}

#[derive(Default)]
pub struct LiveVisuals {
    wanted: BTreeMap<u16, LiveLook>,
    loading: BTreeMap<u16, Loading>,
    drawn: Vec<LiveVisual>,
    failed: BTreeMap<u16, String>,
    loaders: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub now: f64,
    riders: std::sync::Arc<RiderLooks>,
}

impl LiveVisuals {
    pub(super) fn with_riders(riders: RiderLooks) -> Self {
        LiveVisuals { riders: std::sync::Arc::new(riders), ..Default::default() }
    }

    #[cfg(feature = "net")]
    pub fn want(&mut self, id: u16, look: LiveLook) {
        if self.wanted.get(&id) != Some(&look) {
            self.failed.remove(&id);
            self.wanted.insert(id, look);
        }
    }

    #[cfg(feature = "net")]
    pub fn forget(&mut self, id: u16) {
        self.wanted.remove(&id);
        self.failed.remove(&id);
    }

    pub fn drawn(&self) -> usize {
        self.drawn.len()
    }

    pub fn sprays(&self) -> impl Iterator<Item = (u16, JointRecord, &ssx_assets::ghosts::GhostPose)> + '_ {
        self.drawn.iter().filter_map(|r| r.spray.as_ref().map(|(board, pose)| (r.id, *board, pose)))
    }

    pub fn billboards(&self) -> Vec<ssx_ui::game::hud::billboards::Ghost> {
        self.drawn
            .iter()
            .filter_map(|r| {
                let t = r.trajectory?;
                Some(ssx_ui::game::hud::billboards::Ghost {
                    id: PLAYER_IDS + u64::from(r.id),
                    name: r.look.name.clone(),
                    colour: NAME_COLOUR,
                    position: t.p.to_array(),
                })
            })
            .collect()
    }

    pub(super) fn update(
        &mut self,
        race: &ssx_physics::session::bots::BotRace,
        ride: &crate::ride::Ride,
        frame: LiveFrame,
        rng: &mut dyn ssx_assets::career::random::GameRandom,
        draw: &mut RacerDraw,
    ) {
        let LiveFrame { facts, dt_ticks, racers } = frame;
        for r in &mut self.drawn {
            let view = race.live.iter().find(|l| l.id == r.id).and_then(|l| l.pose.view(self.now));
            let Some(view) = view else {
                draw.hide(&r.visual);
                (r.trajectory, r.spray) = (None, None);
                continue;
            };
            let v = &mut r.visual;
            if v.racer_type == 2 {
                draw.holo(v, dt_ticks / 60.0, view.root.distance_squared(ride.state.traj), racers, rng);
            }
            let in_air = view.pose.flags & ssx_formats::ghost::FLAG_IN_AIR != 0;
            v.anim.set_signal("BLN_InAir", if in_air { 1.0 } else { 0.0 });
            let posed = ssx_assets::ghosts::GhostPose { trajectory_rotation: view.rotation, ..view.pose };
            let pose = v.anim.pose(&v.rig, &posed, dt_ticks);
            let mut records = world_records(&v.rig.rig, &pose.nodes, view.root);
            let axe = if r.look.gear & 2 != 0 { 1.0 } else { 0.0 };
            for &op in &v.axes {
                crate::rider::skin::prop_in_hands(&v.rig.rig, &mut records, op, axe);
            }
            let lamp_on = facts.night && r.look.gear & 1 != 0;
            let (board, trajectory) = draw.place(&format!("live rider {}", r.id), v, &records, facts, lamp_on);
            r.trajectory = trajectory;
            r.spray = board.map(|b| (b, view.pose));
        }
    }
}

pub(super) fn spawn_live(
    bots: Option<ResMut<Bots>>,
    root: Option<Res<crate::render::lighting::BigRoot>>,
    mut spawner: RacerSpawner,
) {
    let (Some(mut bots), Some(root)) = (bots, root) else { return };
    let bots = &mut *bots;
    let vault_track = bots.vault_track.clone();
    let live = &mut bots.live;
    let gone: Vec<u16> = live.drawn.iter().map(|r| r.id).filter(|id| !live.wanted.contains_key(id)).collect();
    for id in gone {
        if let Some(k) = live.drawn.iter().position(|r| r.id == id) {
            let r = live.drawn.remove(k);
            despawn(&mut spawner.commands, &r.visual);
            info!("live rider {id} ({}): left", r.look.name);
        }
    }
    live.loading.retain(|id, _| live.wanted.contains_key(id));
    let changed: Vec<u16> =
        live.drawn.iter().filter(|r| live.wanted.get(&r.id) != Some(&r.look)).map(|r| r.id).collect();
    for id in changed {
        if let Some(k) = live.drawn.iter().position(|r| r.id == id) {
            let r = live.drawn.remove(k);
            despawn(&mut spawner.commands, &r.visual);
        }
    }
    let starts: Vec<(u16, LiveLook)> = live
        .wanted
        .iter()
        .filter(|(id, _)| {
            !live.loading.contains_key(id) && !live.failed.contains_key(id) && !live.drawn.iter().any(|r| r.id == **id)
        })
        .take(MAX_LOADS.saturating_sub(live.loaders.load(std::sync::atomic::Ordering::Acquire)))
        .map(|(id, look)| (*id, look.clone()))
        .collect();
    for (id, look) in starts {
        let (tx, rx) = std::sync::mpsc::channel();
        let (root, track, l) = (root.0.clone(), vault_track.clone(), look.clone());
        let before: Vec<u32> = live
            .drawn
            .iter()
            .map(|r| r.look.character)
            .chain(live.loading.values().map(|(l, _)| l.character))
            .collect();
        let riders = live.riders.clone();
        live.loaders.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        let count = LoaderCount(live.loaders.clone());
        let spawned = std::thread::Builder::new().name(format!("live rider {id} loader")).spawn(move || {
            let _count = count;
            let _ = tx.send(load_live(&root, track.as_deref(), &l, &riders, &before));
        });
        match spawned {
            Ok(_) => {
                live.loading.insert(id, (look, Mutex::new(rx)));
            }
            Err(e) => {
                live.failed.insert(id, e.to_string());
            }
        }
    }
    let mut arrived = None;
    for (&id, (look, rx)) in &live.loading {
        let got = match rx.lock() {
            Ok(rx) => rx.try_recv(),
            Err(_) => Err(TryRecvError::Disconnected),
        };
        match got {
            Ok(r) => {
                arrived = Some((id, look.clone(), r));
                break;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                arrived = Some((id, look.clone(), Err(anyhow::anyhow!("the loader ended without a model"))));
                break;
            }
        }
    }
    if let Some((id, look, result)) = arrived {
        live.loading.remove(&id);
        match result {
            Ok(b) => {
                let t0 = std::time::Instant::now();
                let visual = spawner.spawn_racer(bots.board_mesh.as_ref(), b);
                info!(
                    "live rider {id} ({}): drawn as {} ({} style), spawned in {:.2} ms",
                    look.name,
                    visual.name,
                    if look.hologram { "hologram" } else { "character" },
                    t0.elapsed().as_secs_f64() * 1000.0
                );
                live.drawn.push(LiveVisual { id, look, visual, trajectory: None, spray: None });
            }
            Err(e) => {
                warn!("live rider {id} ({}): not drawn: {e:#}", look.name);
                live.failed.insert(id, format!("{e:#}"));
            }
        }
    }
}

fn load_live(
    root: &std::path::Path,
    track: Option<&str>,
    look: &LiveLook,
    level: &RiderLooks,
    before: &[u32],
) -> Result<LoadedBot> {
    let floats = ssx_assets::rig_anim::float_dofs(root)?;
    let db = ssx_assets::anim_db::AssetDb::load(
        &root.join(ssx_assets::rig_anim::ANIM_DIR),
        &[ssx_assets::rig_anim::RIG_BUNDLE],
    )?;
    let content = ssx_assets::content::GameContent::of(root);
    let (name, added) = live_rider(&content, look.character);
    let id_of = |key: u32| {
        let (name, added) = live_rider(&content, key);
        added.or_else(|| ContentId::new(ssx_content::BASE_NAMESPACE, Kind::Character, name).ok())
    };
    let mut ids: Vec<ContentId> = before.iter().filter_map(|&k| id_of(k)).collect();
    ids.extend(id_of(look.character));
    let choice = content.more_rider_looks(level, &ids).1.pop().unwrap_or(LookChoice::Disc);
    let src = RacerSources { root, track, db: &db, floats: &floats };
    load_racer_as(&src, name, look.gear, None, if look.hologram { 2 } else { 1 }, &choice, added.as_ref())
}

fn live_rider(content: &ssx_assets::content::GameContent, key: u32) -> (&'static str, Option<ContentId>) {
    let name = ssx_physics::session::bots::character_name(key);
    if ssx_formats::attrib::fnv1(name.as_bytes()) == key {
        return (name, None);
    }
    match content.session_rider(key) {
        Some((id, stand_in)) => {
            (ssx_physics::session::bots::character_name(ssx_formats::attrib::fnv1(stand_in.as_bytes())), Some(id))
        }
        None => (name, None),
    }
}

fn despawn(commands: &mut Commands, v: &BotVisual) {
    if let Ok(mut e) = commands.get_entity(v.root) {
        e.despawn();
    }
}
