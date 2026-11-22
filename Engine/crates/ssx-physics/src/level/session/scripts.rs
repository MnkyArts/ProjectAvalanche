use super::*;
use std::collections::BTreeSet;
use std::sync::Arc;

impl Session {
    pub fn update_objectives(&mut self, s: &mut RiderState, w: &World, nis_active: bool, dt_s: f32) {
        self.load_objectives(w);
        self.update_objective_sets(s, w, nis_active, dt_s);
        self.update_geotags(s, dt_s);
    }

    pub(super) fn update_geotags(&mut self, s: &RiderState, dt_s: f32) {
        let Some(g) = self.geotags.as_mut() else { return };
        let intro = s.start.as_ref().and_then(|l| l.intro.as_ref());
        let hidden = intro.is_some_and(|i| i.signals().iter().any(|x| x == "BLN_DisableWorldRender"));
        g.update(dt_s, s.traj, hidden);
        if let Some(o) = self.objectives.as_mut() {
            for e in &g.events {
                if let geotags::GeotagEvent::Collected { .. } = e {
                    o.runner.events.post(crate::objectives::Name::new("OfflineGeoCacheCollected"), None);
                }
            }
        }
    }

    pub(super) fn update_objective_sets(&mut self, s: &mut RiderState, w: &World, nis_active: bool, dt_s: f32) {
        let Some(o) = self.objectives.as_mut() else { return };
        let fresh = std::mem::take(&mut self.effects_fresh);
        if o.runner.time > 0.0 && fresh {
            o.effects.clear();
        }
        self.effects_from = if o.runner.time > 0.0 { o.effects.len() } else { 0 };
        let t = self.tutorial.get_or_insert_default();
        let mut world = crate::objectives::SessionWorld {
            s,
            w,
            tracker: &self.tracker,
            event: &mut self.event,
            rewind: &mut self.manager.count,
            nis_active,
            heli_contact: self.heli_jump.as_ref().map(|h| h.contact),
            overlay: &mut o.overlay,
            effects: &mut o.effects,
            heli: &mut o.heli,
            nis_starts: &o.nis_starts,
            intercept: self.heli_jump.as_mut().map(|h| &mut h.intercept),
            tutorial: Some(t),
            unapplied: &mut o.unapplied,
            nis: &self.nis_events,
            postfx: &mut self.postfx,
            backwards: self.rewind_backwards,
            stats: &mut self.stats,
            ui_flags: &self.ui_flags,
            presentation: &mut self.presentation,
        };
        o.runner.update(dt_s, &mut world);
        for (name, v) in &self.flag_overrides {
            o.runner.flags.set(&crate::objectives::Name::new(name), *v, true);
        }
        if let Some(c) = self.controllers.as_mut() {
            let mut ctx =
                crate::controllers::Ctx { sw: &mut world, flags: &mut o.runner.flags, events: &o.runner.events };
            c.update(dt_s, &mut ctx);
        }
        self.screen.update(&o.effects[self.effects_from..], dt_s);
    }

    pub(super) fn new_effects(&self) -> &[(String, crate::objectives::Effect)] {
        self.objectives.as_ref().map_or(&[][..], |o| o.effects.get(self.effects_from..).unwrap_or(&[]))
    }

    pub fn load_objectives(&mut self, w: &World) {
        if !self.objectives_loaded {
            self.nis_events = crate::nis_events::NisEvents::new(w.tuning.posted_events.clone());
            self.objectives_loaded = true;
            let build = match self.scripts_ahead.take() {
                Some(b) if b.made_from(self, w) => {
                    self.scripts_built_ahead = true;
                    b
                }
                _ => ScriptsBuild::build(ScriptsInputs::of(self), w),
            };
            let Some(Built { mut objectives, scripts }) = build.built else { return };
            let event_type = self.event.setup.event_type.map(|t| t as i32);
            self.stats.level_start(&w.survival.slots, event_type);
            if let Some(h) = self.heli_jump.as_mut()
                && objectives.nis_built.contains(ssx_assets::nis::HELI_GRAB_OUTRO_BINDING)
                && let Err(e) = h.read_outro_ahead()
            {
                objectives.report.problems.push(format!("the heli grab's outros: {e:#}"));
            }
            self.objectives = Some(objectives);
            if let Some((scripts, c)) = scripts {
                self.controllers = Some(c);
                self.scripts = Some(scripts);
            }
        }
    }

    pub fn drive_helicopter(&mut self, heli: &mut crate::helicopter::Helicopter) {
        let Some(o) = self.objectives.as_mut() else { return };
        if heli.orders_seen != o.heli.generation {
            (heli.move_to, heli.orders_seen) = (o.heli.move_to, o.heli.generation);
        }
        heli.ends_with_heli_jump = o.heli.ends_with_heli_jump;
        heli.reverse_cam = o.heli.reverse_cam;
        if let Some((p, q)) = o.heli.warp.take() {
            heli.warp(p, q);
        }
    }

    pub fn heli_interaction(
        &self,
        heli: &mut crate::helicopter::Helicopter,
        s: &mut RiderState,
        w: &World,
        tricks: &TrickInput,
    ) {
        use crate::helicopter::interaction::InteractionInputs;
        let stick = if self.pads_released { glam::Vec2::ZERO } else { tricks.right_stick };
        let angle = stick.y.atan2(stick.x).rem_euclid(std::f32::consts::TAU);
        let mag = stick.length().min(1.0);
        let rstick_down = (3.926_991..=5.497_787).contains(&angle) && (0.49..=1.0).contains(&mag);
        let intercept = self.heli_jump.as_ref().is_some_and(|h| h.intercept.valid);
        let x = InteractionInputs {
            dt: DT_TICKS * self.dilation,
            rewind_active: self.buffer.is_active(),
            rewind_terminating: self.buffer.is_terminating(),
            in_air: s.bln_in_air(),
            can_interact: !(intercept || s.state.is_wingsuit()) && s.heli_interaction_allowed,
            aerial_trick_priority: s.tricks.is_tricking && (s.score.is_awesome || s.score.is_super_awesome),
            track_finished: self.flow.track_finished,
            rstick_down,
            goofy: s.goofy,
            ice_axe: w.survival.slots[crate::character::SLOT_ICE_AXE],
            boarder_position: s.traj,
            boarder_velocity: s.traj_velocity,
            drag: crate::intercept::DragModel {
                wind: s.grind.wind,
                kh: w.props.k_h_air,
                kv: w.props.k_v_air,
                g: w.props.g_air.y.abs(),
            },
            heli_position: glam::Vec3::ZERO,
            heli_orientation: glam::Quat::IDENTITY,
            heli_velocity: glam::Vec3::ZERO,
            floor_joint: glam::Vec3::ZERO,
        };
        if let Some(v) = heli.interaction(&mut s.heli, x) {
            s.traj_velocity = v;
        }
    }

    pub fn heli_physics_queries(&mut self, h: &crate::helicopter::Helicopter, w: &World) {
        if let Some(d) = h.physics_queries(&w.collision) {
            self.downwash = Some(d);
            self.set_downwash(&d);
        }
    }

    pub fn interaction_toolkit(&self, heli: Option<&crate::helicopter::Helicopter>, s: &mut RiderState, dt: f32) {
        use crate::query::{Animatable, QueryTask, actor_type, find_heli};
        use std::f32::consts::FRAC_PI_2;
        for task in std::mem::take(&mut s.queued_queries) {
            match task {
                QueryTask::FindHeli => {
                    let player = std::iter::once((Animatable::Player, actor_type::SNOWBOARDER));
                    let helicopter = heli.map(|_| (Animatable::Helicopter, actor_type::HELICOPTER));
                    let ghosts = self
                        .bots
                        .iter()
                        .flat_map(|b| (0..b.bots.len()).map(|k| (Animatable::Ghost(k), actor_type::GHOST)));
                    s.target_animatable = find_heli(Animatable::Player, player.chain(helicopter).chain(ghosts));
                }
            }
        }
        if let Some(d) = s.pin.as_mut() {
            let signals = crate::driver::PinSignals { position: s.traj_cached, orientation: s.traj_orientation };
            if let Some((p, q)) = d.update(dt, signals) {
                s.traj = p.unwrap_or(s.traj);
                s.traj_orientation = q.unwrap_or(s.traj_orientation);
            }
        }
        let Some(d) = s.attachment.as_mut() else { return };
        let (Some(Animatable::Helicopter), Some(h)) = (s.target_animatable, heli) else { return };
        let heli_rig = ssx_assets::nis::HELI_RIG;
        let Some((_, joint)) = d.tag.target_joints.iter().find(|(rig, _)| rig == heli_rig) else { return };
        let Some(target) = h.joint_world(joint) else { return };
        let root = (s.traj_orientation * glam::Quat::from_rotation_y(-FRAC_PI_2), s.traj);
        if let Some((q, p)) = d.update(root, target, dt) {
            s.traj_orientation = (q * glam::Quat::from_rotation_y(FRAC_PI_2)).normalize();
            s.traj = p;
        }
    }

    pub fn set_downwash(&mut self, d: &crate::helicopter::downwash::Downwash) {
        use crate::helicopter::downwash::{ID_DOWNWASH_GROUND_AO, ID_DOWNWASH_VISUAL_STRENGTH};
        use ssx_lua::game::objectives::GsValue;
        let Some(o) = self.objectives.as_mut() else { return };
        let heli = crate::objectives::Name::new("DropHeli").hash;
        o.overlay.insert((heli, ID_DOWNWASH_VISUAL_STRENGTH), GsValue::Float(d.visual_strength));
        o.overlay.insert((heli, ID_DOWNWASH_GROUND_AO), GsValue::Float(d.ground_ao));
    }

    pub fn restart_objectives(&mut self, s: &mut RiderState, w: &World, nis_active: bool) {
        let Some(o) = self.objectives.as_mut() else { return };
        let mut world = crate::objectives::SessionWorld {
            s,
            w,
            tracker: &self.tracker,
            event: &mut self.event,
            rewind: &mut self.manager.count,
            nis_active,
            heli_contact: self.heli_jump.as_ref().map(|h| h.contact),
            overlay: &mut o.overlay,
            effects: &mut o.effects,
            heli: &mut o.heli,
            nis_starts: &o.nis_starts,
            intercept: self.heli_jump.as_mut().map(|h| &mut h.intercept),
            tutorial: self.tutorial.as_mut(),
            unapplied: &mut o.unapplied,
            nis: &self.nis_events,
            postfx: &mut self.postfx,
            backwards: self.rewind_backwards,
            stats: &mut self.stats,
            ui_flags: &self.ui_flags,
            presentation: &mut self.presentation,
        };
        o.runner.restart_set(crate::objectives::SetId::Level, &mut world);
    }
}

// PORT(choice): a caller may build it ahead on another thread ([`ScriptsBuild::ahead`], `Session::scripts_ahead`);
// the level's first execution takes it only when it was made from the world the execution gets (the same shared
// world, which nothing changes while it is shared), from the session's inputs as they are then and under the content
// state the NIS bindings are read under then (the data folder's generation and simulation policy, unchanged during
// the build), else builds it as before, so the objects are the same. Built in that execution it took 10.9–19.3 ms
// (alps0101, Mount Eddie, the heli scene, an online race; the objectives' sets A and B 6.9–12.8 ms, the batches
// 2.2–4.0, the controllers 1.6–3.5; untraced, development laptop). The console built them in the first update of
// `FMGameScripting` (0x82379D40). The Lua VM's one thread state is `math.random`'s generator, which no disc or DLC
// script calls; the build draws nothing from the game's global generator.
#[derive(Clone)]
pub struct ScriptsBuild {
    inputs: ScriptsInputs,
    ahead: Option<Ahead>,
    built: Option<Built>,
}

impl std::fmt::Debug for ScriptsBuild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ahead = self.ahead.as_ref().map(|a| (Arc::as_ptr(&a.world), &a.content));
        f.debug_struct("ScriptsBuild")
            .field("inputs", &self.inputs)
            .field("ahead", &ahead)
            .field("built", &self.built)
            .finish()
    }
}

#[derive(Clone)]
struct Ahead {
    world: Arc<World>,
    content: Option<ContentRead>,
}

#[derive(Clone, Debug, PartialEq)]
struct ContentRead {
    root: Option<std::path::PathBuf>,
    state: Option<(u64, ssx_content::policy::ClientMods)>,
}

impl ContentRead {
    fn now(root: Option<std::path::PathBuf>) -> Self {
        let state = root.as_deref().map(|r| {
            let content = ssx_assets::content::GameContent::of(r);
            (content.generation(), content.sim_policy())
        });
        Self { root, state }
    }
}

#[derive(Clone, Debug)]
struct Built {
    objectives: crate::objectives::LevelObjectives,
    scripts: Option<(Arc<crate::objectives::LevelScripts>, crate::controllers::Controllers)>,
}

impl ScriptsBuild {
    pub fn ahead(inputs: ScriptsInputs, w: Arc<World>) -> Self {
        let before = ContentRead::now(Self::root(&inputs, &w));
        let mut b = Self::build(inputs, &w);
        let content = (ContentRead::now(before.root.clone()) == before).then_some(before);
        b.ahead = Some(Ahead { world: w, content });
        b
    }

    fn root(inputs: &ScriptsInputs, w: &World) -> Option<std::path::PathBuf> {
        let event = inputs.event.as_ref().map(|e| &e.scripts);
        w.survival.objectives.as_ref().and_then(|d| d.root.clone()).or_else(|| event.and_then(|e| e.root.clone()))
    }

    fn build(inputs: ScriptsInputs, w: &World) -> Self {
        let event = inputs.event.as_ref().map(|e| &e.scripts);
        let drop = w.survival.objectives.as_deref();
        let batch = match (drop, event) {
            (Some(d), e) => d.level_batch(e),
            (None, Some(e)) => e.batch(),
            (None, None) => Vec::new(),
        };
        if batch.is_empty() {
            return Self { inputs, ahead: None, built: None };
        }
        let root = Self::root(&inputs, w);
        let game = drop.map_or_else(Vec::new, |d| d.named(&crate::objectives::GAME_SCRIPTS));
        let seed: Vec<u32> = inputs.seed.iter().copied().collect();
        let (slots, event_type, message) = (w.survival.slots, inputs.event_type, inputs.message.clone());
        let load = crate::objectives::LoadWorld { slots, event_type, message, effects: Vec::new() };
        let log = inputs.objectives_log;
        let objectives = crate::objectives::LevelObjectives::load(&game, &batch, load, log, root.as_deref(), &seed);
        let scripts = drop.map(|d| {
            let scripts = crate::objectives::LevelScripts::load(d, event);
            let (sets, scene, schemas, lamp) =
                (&scripts.state_sets, &scripts.scene, &scripts.schemas, w.survival.head_lamp_scale);
            let mut c = crate::controllers::Controllers::load(sets, scene, schemas, &w.triggers, lamp);
            c.log = inputs.controllers_log.then(Vec::new);
            for e in &inputs.named_entities {
                if let Some(x) = e.controller() {
                    c.add_entity(x);
                }
            }
            (Arc::new(scripts), c)
        });
        Self { inputs, ahead: None, built: Some(Built { objectives, scripts }) }
    }

    pub fn made_from(&self, s: &Session, w: &World) -> bool {
        self.ahead.as_ref().is_some_and(|a| {
            std::ptr::eq(Arc::as_ptr(&a.world), w)
                && a.content.as_ref().is_some_and(|c| ContentRead::now(c.root.clone()) == *c)
        }) && self.inputs.matches(s)
    }
}

#[derive(Clone, Debug)]
pub struct ScriptsInputs {
    event: Option<Arc<crate::objectives::EventObjectives>>,
    event_type: Option<i32>,
    message: crate::objectives::DropMessage,
    seed: BTreeSet<u32>,
    objectives_log: bool,
    named_entities: Vec<crate::controllers::NamedEntity>,
    controllers_log: bool,
}

impl ScriptsInputs {
    pub fn of(s: &Session) -> Self {
        Self {
            event: s.event.setup.objectives.clone(),
            event_type: s.event.setup.event_type.map(|t| t as i32),
            message: s.event.setup.message.clone(),
            seed: s.named_flag_seed.clone(),
            objectives_log: s.objectives_log,
            named_entities: s.named_entities.clone(),
            controllers_log: s.controllers_log,
        }
    }

    fn matches(&self, s: &Session) -> bool {
        let Self { event, event_type, message, seed, objectives_log, named_entities, controllers_log } = self;
        let same_event = match (event, &s.event.setup.objectives) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            (None, None) => true,
            _ => false,
        };
        same_event
            && *event_type == s.event.setup.event_type.map(|t| t as i32)
            && *message == s.event.setup.message
            && *seed == s.named_flag_seed
            && *objectives_log == s.objectives_log
            && *named_entities == s.named_entities
            && *controllers_log == s.controllers_log
    }
}
