use std::path::Path;

use anyhow::{Context, Result};

pub const SCRIPTS_DIR: &str = "memresident/data/common/scripts";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneOrigin {
    Absolute,
    LevelStart,
    LevelEnd,
    Locator,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntroVariant {
    Heli,
    Snow,
    Outro,
}

#[derive(Clone, Debug, PartialEq)]
pub struct IntroChoice {
    pub script: String,
    pub binding: String,
    pub origin: SceneOrigin,
    pub locator_alternate: Option<String>,
    pub variant: IntroVariant,
}

pub fn intro_of_probe(p: &crate::objectives::Probe) -> Option<IntroChoice> {
    use crate::objectives::{Effect, GsValue, game_state_name};
    let (objective, r) = p.first_nis()?;
    let snow = p.before_first_nis().iter().any(|(_, e)| {
        matches!(e, Effect::SetGameState { entity, id, value: GsValue::Bool(true) }
            if entity.is("Player1") && game_state_name(*id) == "BLN_NisSnowStart")
    });
    let script = p.scripts.get(objective).map_or("", String::as_str);
    let origin = match r.origin {
        0 => SceneOrigin::Absolute,
        1 => SceneOrigin::LevelStart,
        2 => SceneOrigin::LevelEnd,
        _ => SceneOrigin::Locator,
    };
    Some(IntroChoice {
        script: script.rsplit(['/', '\\']).next().unwrap_or(script).to_string(),
        binding: r.name.clone(),
        origin,
        locator_alternate: r.locator_alternate.as_ref().map(|n| n.text.to_string()),
        variant: if snow { IntroVariant::Snow } else { IntroVariant::Heli },
    })
}

pub fn intro_of_script(name: &str, text: &str) -> Option<IntroChoice> {
    let slots = crate::character::GearStats::default_gear().slots;
    intro_of_probe(&crate::objectives::Probe::run(&[(name.to_string(), text.as_bytes().to_vec())], None, slots))
}

pub fn intro_of_event(big_root: &Path, event: &crate::event::EventDef) -> Result<Option<IntroChoice>> {
    match &event.objectives {
        Some(o) => Ok(intro_of_probe(&o.probe)),
        None => intro_of_scripts(big_root, &event.scripts),
    }
}

pub fn intro_of_scripts(big_root: &Path, scripts: &[String]) -> Result<Option<IntroChoice>> {
    let d = crate::objectives::DropScripts::of(big_root, scripts);
    let slots = crate::character::GearStats::default_gear().slots;
    Ok(intro_of_probe(&crate::objectives::Probe::run(&d.batch(), None, slots)))
}

mod pose;
mod side;

pub use side::{SceneFade, SceneRef, Side, random_pick};

use std::sync::Arc;

use glam::{Quat, Vec3};
pub use ssx_assets::nis::IntroPath;
use ssx_assets::nis::{IntroNis, NisActor, NisScene, ROLE_CHARACTER1};

use crate::tags::{TagEvent, TagTracker, TagWindow};

#[derive(Debug)]
pub struct IntroAssets {
    pub choice: IntroChoice,
    pub nis: IntroNis,
    pub jump_clip: usize,
    pub jump_clips: Vec<usize>,
    pub event: String,
    pub heli: Result<ssx_assets::helicopter::HeliChoice, String>,
}

impl IntroAssets {
    pub fn scene(&self, s: Scene) -> &NisScene {
        match s {
            Scene::Ready { .. } => &self.nis.ready,
            Scene::Jump { .. } => &self.nis.jump,
        }
    }

    pub fn sequence_time(&self, s: Scene) -> f32 {
        let ticks = match s {
            Scene::Ready { ticks } => ticks,
            Scene::Jump { ticks } => self.nis.jump_start + ticks,
        };
        match self.scene(s).loop_end {
            Some(end) if end > 0.0 => ticks.rem_euclid(end),
            _ => ticks,
        }
    }
}

pub fn prepare(
    world: &mut crate::rider::World,
    big_root: &Path,
    event: &crate::event::EventDef,
    character: &str,
) -> Result<Option<Arc<IntroAssets>>> {
    prepare_path(world, big_root, event, character, IntroPath::Short)
}

pub fn prepare_path(
    world: &mut crate::rider::World,
    big_root: &Path,
    event: &crate::event::EventDef,
    character: &str,
    path: IntroPath,
) -> Result<Option<Arc<IntroAssets>>> {
    let Some(choice) = intro_of_event(big_root, event)? else { return Ok(None) };
    let heli = ssx_assets::helicopter::drop_helicopter(big_root, &event.name).map_err(|e| format!("{e:#}"));
    prepare_choice(world, big_root, choice, heli, &event.name, character, path).map(Some)
}

pub fn prepare_choice(
    world: &mut crate::rider::World,
    big_root: &Path,
    choice: IntroChoice,
    heli: Result<ssx_assets::helicopter::HeliChoice, String>,
    event: &str,
    character: &str,
    path: IntroPath,
) -> Result<Arc<IntroAssets>> {
    let snow = choice.variant == IntroVariant::Snow;
    let reverse = world.survival.avalanche.reverse_cam;
    let h = heli.as_ref().ok().map(|h| h.name.as_str());
    let mut nis = ssx_assets::nis::load_drop_intro(big_root, &choice.binding, character, h, snow, path, reverse)?;
    snow_picks(&mut nis)?;
    let rider = nis.jump.actor(ROLE_CHARACTER1).context("the go scene has no Character1 actor")?;
    let clips: Vec<&ssx_assets::nis::NisClip> = match rider.choices.len() > 1 {
        true => rider.choices.iter().map(|c| &c.0).collect(),
        false => vec![&rider.clip],
    };
    let mut jump_clips = Vec::with_capacity(clips.len());
    for c in &clips {
        jump_clips.push(match c.delta_trajectory {
            true => world.tuning.root_motion.push_go(crate::rootmotion::RootClip::from_nis(c)?),
            false => usize::MAX,
        });
    }
    let first = clips.iter().position(|c| c.name == rider.clip.name).unwrap_or(0);
    let jump_clip = jump_clips[first];
    Ok(Arc::new(IntroAssets { choice, nis, jump_clip, jump_clips, event: event.to_string(), heli }))
}

pub fn profile_path(
    big_root: &Path,
    event: &crate::event::EventDef,
    visits: u32,
    wingsuit_enabled: bool,
) -> Result<IntroPath> {
    let Some(choice) = intro_of_event(big_root, event)? else { return Ok(IntroPath::Short) };
    let requires = ssx_assets::nis::requires_wingsuit(big_root, &choice.binding)?;
    Ok(IntroPath::decide(false, false, requires, wingsuit_enabled, visits))
}

// TODO(guess): the rider's random value is fresh (seeded) at every intro, as `levelstart::go_pick` takes it (on a level's first start nothing draws on it before: its users are the 151 random choosers of the disc, all in NIS / level-start graphs, and the camera's `CameraRandomVector3`); a restart or the survive loop's next intro may continue the run before's draws; settle: whether the rider actor's `RandomValue` state is rebuilt at a restart and at the survive loop's next intro.
pub fn snow_picks(nis: &mut IntroNis) -> Result<()> {
    let Some(seed) = nis.ready.actor(ROLE_CHARACTER1).filter(|a| !a.sequences.is_empty()).and_then(|a| a.seed) else {
        return Ok(());
    };
    crate::guess_hit!();
    let mut state = seed;
    let pick = |state: &mut u32, a: &NisActor| random_pick(state, &a.choices.iter().map(|c| c.1).collect::<Vec<_>>());
    let ready = nis.ready.actors.iter_mut().find(|a| a.role == ROLE_CHARACTER1).context("no Character1")?;
    let k = pick(&mut state, ready);
    ready.clip = ready.choices[k].0.clone();
    ready.length = ready.sequences[k].1;
    nis.ready.sequence = ready.sequences[k].0.clone();
    if let Some(go) = nis.jump.actors.iter_mut().find(|a| a.role == ROLE_CHARACTER1 && a.choices.len() > 1) {
        let k = pick(&mut state, go);
        go.clip = go.choices[k].0.clone();
        nis.jump.sequence = go.clip.name.clone();
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scene {
    Ready { ticks: f32 },
    Jump { ticks: f32 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RoundHold {
    #[default]
    Off,
    Armed,
    Held {
        go: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CabinClock {
    pub index: usize,
    pub ticks: f32,
}

pub type Rt = (Quat, Vec3);

fn compose(a: Rt, b: Rt) -> Rt {
    ((a.0 * b.0).normalize(), a.1 + a.0 * b.1)
}

fn inverse(a: Rt) -> Rt {
    let q = a.0.conjugate();
    (q, -(q * a.1))
}

fn rt(q: [f32; 4], t: [f32; 3]) -> Rt {
    (Quat::from_array(q).normalize(), Vec3::from_array(t))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NisView {
    pub position: Vec3,
    pub rotation: Quat,
    pub hfov_deg: f32,
    pub near: Option<f32>,
    pub trajectory: Rt,
    pub eye: Rt,
}

#[derive(Clone, Debug)]
pub struct IntroState {
    pub assets: Arc<IntroAssets>,
    pub position: Vec3,
    pub orientation: Quat,
    pub scene: Scene,
    pub cabin: Option<CabinClock>,
    pub tuning: Vec3,
    pub alternate: Rt,
    pub dt: f32,
    pub attached: bool,
    drivers: TagTracker,
    stop: TagTracker,
    game: TagTracker,
    game_events: Vec<(usize, TagEvent)>,
    pub rider_pick: usize,
    pub camera_pick: usize,
    rider_random: u32,
    fades: Vec<SceneFade>,
    ghost_root: Option<(Rt, SceneRef, f32)>,
    hold: RoundHold,
    fresh: bool,
}

impl PartialEq for IntroState {
    fn eq(&self, o: &Self) -> bool {
        Arc::ptr_eq(&self.assets, &o.assets)
            && (self.position, self.orientation, self.scene, self.dt, self.attached, self.hold)
                == (o.position, o.orientation, o.scene, o.dt, o.attached, o.hold)
    }
}

fn windows(ws: impl Iterator<Item = ssx_assets::nis::Window>) -> Vec<TagWindow> {
    ws.map(|w| TagWindow::new(w.time, w.duration, w.always_active)).collect()
}

impl IntroState {
    pub fn new(
        assets: Arc<IntroAssets>,
        world: &crate::rider::World,
        position: Vec3,
        orientation: Quat,
    ) -> Option<Self> {
        let at = crate::levelstart::offset_level_start(world, position).unwrap_or(position);
        let orientation = orientation.normalize();
        let alternate = assets
            .choice
            .locator_alternate
            .as_deref()
            .and_then(|n| crate::outro::locator(&world.triggers, n))
            .map_or((orientation, position), |(p, q)| (q.normalize(), p));
        let cabin = (!assets.nis.cabin.is_empty()).then_some(CabinClock { index: 0, ticks: 0.0 });
        let direct = cabin.is_none() && !assets.nis.ready_loop;
        let first = match cabin {
            Some(_) => &assets.nis.cabin[0].scene,
            None if direct => &assets.nis.jump,
            None => &assets.nis.ready,
        };
        let rider = first.actor(ROLE_CHARACTER1)?;
        let drivers = TagTracker::new(&windows(rider.tags.drivers.iter().map(|d| d.window)));
        let attached = !rider.tags.drivers.is_empty();
        let tuning = match cabin {
            Some(_) => assets.nis.cabin[0].tuning_offset,
            None => first.node_tags.tuning_offset,
        };
        let stop = TagTracker::new(&windows(rider.tags.stop.into_iter().filter(|_| direct)));
        let game = Self::game_tracker(rider);
        let seed = assets.nis.ready.actor(ROLE_CHARACTER1).and_then(|a| a.seed);
        let mut rider_random =
            seed.or_else(|| assets.nis.jump.actor(ROLE_CHARACTER1).and_then(|a| a.seed)).unwrap_or(0);
        let mut rider_pick = 0;
        if let Some(a) = assets.nis.ready.actor(ROLE_CHARACTER1).filter(|a| a.choices.len() > 1 && seed.is_some()) {
            rider_pick = random_pick(&mut rider_random, &a.choices.iter().map(|c| c.1).collect::<Vec<_>>());
        }
        Some(Self {
            scene: if direct { Scene::Jump { ticks: 0.0 } } else { Scene::Ready { ticks: 0.0 } },
            assets,
            position: at,
            orientation,
            cabin,
            tuning: tuning.map_or(Vec3::ZERO, Vec3::from_array),
            alternate,
            dt: 0.0,
            attached,
            drivers,
            stop,
            game,
            game_events: Vec::new(),
            rider_pick,
            camera_pick: 0,
            rider_random,
            fades: Vec::new(),
            ghost_root: None,
            hold: RoundHold::Off,
            fresh: true,
        })
        .map(|mut x| {
            x.update_ghost();
            x
        })
    }

    fn game_tracker(a: &NisActor) -> TagTracker {
        TagTracker::new(&windows(a.tags.game_state.iter().map(|t| t.window)))
    }

    pub fn game_state_events(&self) -> Vec<(&ssx_assets::nis::GameStateTag, TagEvent)> {
        let Some(a) = self.actor(ROLE_CHARACTER1) else { return Vec::new() };
        self.game_events.iter().filter_map(|&(i, e)| Some((a.tags.game_state.get(i)?, e))).collect()
    }

    pub fn in_ready_loop(&self) -> bool {
        self.cabin.is_none() && matches!(self.scene, Scene::Ready { .. })
    }

    pub fn waits_for_go(&self) -> bool {
        self.in_ready_loop() || matches!(self.hold, RoundHold::Held { .. })
    }

    pub fn hold_for_round(&mut self) -> bool {
        if self.assets.nis.ready_loop {
            return true;
        }
        let started = self.cabin.is_none() && !self.fresh;
        if self.hold == RoundHold::Off && !started {
            self.hold = RoundHold::Armed;
        }
        self.hold != RoundHold::Off
    }

    fn hold_at_entry(&mut self) {
        // TODO(guess): a round holds a way without a ready loop at its last scene's entry, the whole ride standing (no
        // execution: the rider attached in the helicopter at the scene's first frame, the NIS camera and the helicopter
        // still) until the round's GO, which plays the scene from its start; the game has no round start (single
        // player's last scene runs on from the cabin scenes and hands over by its stop tag, 40 to 510 ticks; no GO,
        // `BLN_IsNisReadyLoop` never set) and the scene no loop to wait in; settle: the user's choice of the wait's
        // picture (this frozen entry, or another of the scene's frames).
        crate::guess_hit!();
        self.hold = RoundHold::Held { go: false };
    }

    pub fn held(&self) -> bool {
        self.hold == RoundHold::Held { go: false }
    }

    pub fn round_go(&mut self) {
        if let RoundHold::Held { go } = &mut self.hold {
            *go = true;
        }
    }

    pub fn round_hold(&self) -> RoundHold {
        self.hold
    }

    pub fn signals(&self) -> &[String] {
        &self.current().active_signals
    }

    pub fn current(&self) -> &NisScene {
        match self.cabin {
            Some(c) => &self.assets.nis.cabin[c.index].scene,
            None => self.assets.scene(self.scene),
        }
    }

    pub fn ticks(&self) -> f32 {
        if let Some(c) = self.cabin {
            return c.ticks;
        }
        match self.scene {
            Scene::Ready { ticks } | Scene::Jump { ticks } => ticks,
        }
    }

    pub fn sequence_time(&self, ticks: f32) -> f32 {
        if let Some(c) = self.cabin {
            return self.assets.nis.cabin[c.index].start + ticks;
        }
        let s = match self.scene {
            Scene::Ready { .. } => Scene::Ready { ticks },
            Scene::Jump { .. } => Scene::Jump { ticks },
        };
        self.assets.sequence_time(s)
    }

    fn actor(&self, role: i64) -> Option<&NisActor> {
        self.current().actor(role)
    }

    pub fn branch_due(&self) -> bool {
        self.cabin.is_none()
            && matches!(self.scene, Scene::Ready { ticks } if self.assets.nis.branch_out.is_some_and(|t| ticks >= t))
    }

    pub fn root_motion(&self) -> bool {
        matches!(self.scene, Scene::Jump { .. })
            && !self.attached
            && self.actor(ROLE_CHARACTER1).is_some_and(|a| a.clip.delta_trajectory)
    }

    pub fn transition(&mut self) {
        if self.cabin.is_none()
            && let Scene::Ready { .. } = self.scene
        {
            let blend = self.assets.nis.transition_blend.clone();
            self.push_fade(&blend);
            self.enter_jump(0.0);
        }
    }

    fn enter_jump(&mut self, ticks: f32) {
        self.scene = Scene::Jump { ticks };
        self.go_pick();
        let jump = &self.assets.nis.jump;
        if let Some(t) = jump.node_tags.tuning_offset {
            self.tuning = Vec3::from_array(t);
        }
        let rider = jump.actor(ROLE_CHARACTER1);
        self.drivers =
            TagTracker::new(&windows(rider.into_iter().flat_map(|a| a.tags.drivers.iter().map(|d| d.window))));
        self.stop = TagTracker::new(&windows(rider.and_then(|a| a.tags.stop).into_iter()));
        self.game = rider.map_or_else(|| TagTracker::new(&[]), Self::game_tracker);
    }

    pub fn advance(&mut self, dt: f32) {
        self.fresh = false;
        match self.hold {
            RoundHold::Held { go: false } => return,
            RoundHold::Held { go: true } => self.hold = RoundHold::Off,
            RoundHold::Armed if self.cabin.is_none() && matches!(self.scene, Scene::Jump { .. }) => {
                self.hold_at_entry();
                self.update_tags(0.0);
                return;
            }
            RoundHold::Armed | RoundHold::Off => {}
        }
        if self.cabin.is_some() {
            self.advance_cabin(dt);
        } else {
            match (self.scene, self.ready_pick_length()) {
                (Scene::Ready { ticks }, Some(len)) if len - ticks <= dt => {
                    self.update_fades((len - ticks).max(0.0));
                    let rest = dt - (len - ticks).max(0.0);
                    self.ready_rechoose(rest);
                    self.update_fades(rest);
                }
                _ => {
                    match &mut self.scene {
                        Scene::Ready { ticks } | Scene::Jump { ticks } => *ticks += dt,
                    }
                    self.update_fades(dt);
                }
            }
        }
        self.update_tags(dt);
    }

    fn update_tags(&mut self, dt: f32) {
        self.dt = dt;
        self.update_ghost();
        let t = self.sequence_time(self.ticks());
        self.drivers.update(t);
        self.attached = self.drivers.process().iter().any(|(_, e)| matches!(e, TagEvent::Apply(_)));
        self.game_events = if self.game.update(t) { self.game.process() } else { Vec::new() };
    }

    fn advance_cabin(&mut self, dt: f32) {
        let Some(mut c) = self.cabin else { return };
        let mut rest = dt;
        let assets = self.assets.clone();
        let cabin = &assets.nis.cabin;
        loop {
            let sc = &cabin[c.index];
            let left = (sc.end - (sc.start + c.ticks)).max(0.0);
            if rest <= left {
                c.ticks += rest;
                self.cabin = Some(c);
                self.update_fades(rest);
                return;
            }
            rest -= left;
            c.ticks += left;
            self.cabin = Some(c);
            self.update_fades(left);
            self.push_fade(&sc.blend);
            if c.index + 1 < cabin.len() {
                self.enter_cabin(c.index + 1);
                c = CabinClock { index: c.index + 1, ticks: 0.0 };
            } else {
                self.cabin = None;
                if !self.assets.nis.ready_loop && self.hold == RoundHold::Armed {
                    self.hold_at_entry();
                    self.enter_jump(0.0);
                    self.update_fades(0.0);
                    return;
                }
                if !self.assets.nis.ready_loop {
                    self.enter_jump(rest);
                } else {
                    self.enter_ready(rest);
                }
                self.update_fades(rest);
                return;
            }
        }
    }

    pub fn stop_request(&mut self) -> bool {
        match self.scene {
            Scene::Jump { ticks } => {
                self.stop.update(self.assets.sequence_time(Scene::Jump { ticks }));
                self.stop.process().iter().any(|(_, e)| matches!(e, TagEvent::Apply(_)))
            }
            Scene::Ready { .. } => false,
        }
    }

    pub fn outro_active(&self) -> bool {
        if self.current().active_signals.iter().any(|s| s == "BLN_NisOutroActive") {
            return true;
        }
        let t = self.sequence_time(self.ticks());
        self.actor(ROLE_CHARACTER1)
            .and_then(|a| a.tags.outro_active)
            .is_some_and(|w| w.always_active || (t >= w.time && t <= w.time + w.duration))
    }

    pub fn takes_go(&self) -> bool {
        let tags = |a: Option<&NisActor>| a.is_some_and(|a| a.tags.runs_nis_script(0));
        tags(self.actor(ROLE_CHARACTER1)) || self.current().node_tags.runs_nis_script(0)
    }

    pub fn takes_skip(&self) -> bool {
        let t = self.sequence_time(self.ticks());
        let active = |tags: &ssx_assets::nis::Tags, t: f32| {
            tags.scripts.iter().any(|c| {
                c.script == "script_nis"
                    && c.update == 1
                    && (c.window.always_active || (t >= c.window.time && t <= c.window.time + c.window.duration))
            })
        };
        self.actor(ROLE_CHARACTER1).is_some_and(|a| active(&a.tags, t))
            || active(&self.current().node_tags, (self.ticks() - self.dt).max(0.0))
    }

    pub fn skip(&mut self) -> bool {
        let Some(c) = self.cabin else { return false };
        let assets = self.assets.clone();
        let Some((to, blend)) = &assets.nis.cabin[c.index].skip else { return false };
        self.push_fade(blend);
        match *to {
            ssx_assets::nis::SkipTo::Cabin(i) => self.enter_cabin(i),
            ssx_assets::nis::SkipTo::ReadyLoop => self.enter_ready(0.0),
            ssx_assets::nis::SkipTo::Jump => {
                self.cabin = None;
                self.enter_jump(0.0);
            }
        }
        true
    }

    fn enter_cabin(&mut self, index: usize) {
        self.cabin = Some(CabinClock { index, ticks: 0.0 });
        let next = &self.assets.nis.cabin[index];
        if let Some(t) = next.tuning_offset {
            self.tuning = Vec3::from_array(t);
        }
        let rider = next.scene.actor(ROLE_CHARACTER1);
        self.drivers =
            TagTracker::new(&windows(rider.into_iter().flat_map(|a| a.tags.drivers.iter().map(|d| d.window))));
        self.game = rider.map_or_else(|| TagTracker::new(&[]), Self::game_tracker);
    }

    fn enter_ready(&mut self, ticks: f32) {
        self.cabin = None;
        self.scene = Scene::Ready { ticks };
        let ready = &self.assets.nis.ready;
        if let Some(t) = ready.node_tags.tuning_offset {
            self.tuning = Vec3::from_array(t);
        }
        let rider = ready.actor(ROLE_CHARACTER1);
        self.drivers =
            TagTracker::new(&windows(rider.into_iter().flat_map(|a| a.tags.drivers.iter().map(|d| d.window))));
        self.game = rider.map_or_else(|| TagTracker::new(&[]), Self::game_tracker);
    }
}

pub fn level_start(
    world: &crate::rider::World,
    position: Vec3,
    orientation: Quat,
    assets: Arc<IntroAssets>,
) -> Option<crate::rider::RiderState> {
    level_start_on(world, position, position, orientation, assets)
}

pub fn level_start_or_below(
    world: &crate::rider::World,
    position: Vec3,
    orientation: Quat,
    assets: Arc<IntroAssets>,
) -> Option<crate::rider::RiderState> {
    level_start(world, position, orientation, assets.clone()).or_else(|| {
        let hit = world.collision.ray_cast(position, -Vec3::Y, 20_000.0)?;
        level_start_on(world, hit.point + Vec3::Y, position, orientation, assets)
    })
}

pub fn level_start_on(
    world: &crate::rider::World,
    ground: Vec3,
    position: Vec3,
    orientation: Quat,
    assets: Arc<IntroAssets>,
) -> Option<crate::rider::RiderState> {
    let mut s = crate::rider::RiderState::level_start(world, ground, orientation)?;
    s.level_start = position;
    let ls = crate::levelstart::LevelStart::new(world, position, orientation)?.with_intro(world, assets)?;
    let (traj, t) = ls.idle_pose(&world.tuning.root_motion)?;
    (s.traj, s.traj_cached, s.traj_cached_prev) = (traj, traj, traj);
    (s.alignment, s.traj_orientation, s.heading) = (t, t, t * Vec3::X);
    s.janked = crate::janked::JankedState::new(traj);
    s.grind.tail_pos = traj;
    s.start = Some(ls);
    s.position = s.board_point();
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intro_of_a_script() {
        let race = r#"BeginObjective("Race Intro NIS")
	DO_PlayNis(
	{
		name = "nis_binding_intro_race",
		sceneOrigin = NIS_SCENE_ORIGIN_LEVEL_START,
		locatorAlternate = "nis_intro_locator",
		actors = { {role = "Character",  entity = "Player1"} }
	})
EndObjective()"#;
        let i = intro_of_script("eventtype_race.lua", race).expect("intro");
        assert_eq!(i.binding, "nis_binding_intro_race");
        assert_eq!(i.origin, SceneOrigin::LevelStart);
        assert_eq!(i.locator_alternate.as_deref(), Some("nis_intro_locator"));
        assert_eq!(i.variant, IntroVariant::Heli);
        let snow = format!(
            "BeginObjective(\"x\")\n\tDO_SetGameStateValue(\"Player1\", \tBLN_NisSnowStart, true)\n{}",
            &race[race.find("\tDO_PlayNis").unwrap_or(0)..]
        );
        assert_eq!(intro_of_script("eventtype_headtohead.lua", &snow).map(|i| i.variant), Some(IntroVariant::Snow));
        assert!(intro_of_script("x.lua", "BeginObjective(\"none\")\nEndObjective()").is_none());
    }

    #[test]
    fn attachment_places_the_source_on_the_target() {
        let target: Rt = (Quat::from_rotation_y(0.7) * Quat::from_rotation_x(0.2), Vec3::new(3.0, 12.0, -1.0));
        let src: Rt = (Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2), Vec3::new(0.01, -0.86, 1.92));
        let root = compose(target, inverse(src));
        let back = compose(root, src);
        assert!(back.1.distance(target.1) < 1e-5 && back.0.dot(target.0).abs() > 1.0 - 1e-6);
    }
}
