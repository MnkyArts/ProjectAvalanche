use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use bevy::prelude::{Mat4, Quat, Vec3, Vec4};
use ssx_assets::particle_script;
use ssx_assets::particles::{self, ParticleNode};
use ssx_assets::track_objects::Placement;
use ssx_formats::lynx::{Behaviour, ParticleSystem};
use ssx_physics::controllers::{Controllers, EntityController, OfflineHost, ParticleTarget, Switch, Target};

use crate::particles::Scope;
use crate::particles::inputs::Script;
use crate::particles::sim::{EffectDef, EffectInstance, RecordView, SetArena, Visibility};

#[derive(Default)]
pub struct DefCache {
    defs: HashMap<(String, String), Arc<EffectDef>>,
    files: HashMap<String, Arc<ParticleSystem>>,
    behaviours: HashMap<String, Option<Arc<Script>>>,
}

impl DefCache {
    pub fn node(&mut self, root: &Path, node: &ParticleNode) -> Result<(Arc<EffectDef>, Option<Arc<Script>>)> {
        let vfx = node.lynx.as_ref().with_context(|| format!("ge_particlenode {:#010x} has no effect", node.key))?;
        let key = (vfx.path.clone(), vfx.definition.clone());
        let def = match self.defs.get(&key) {
            Some(d) => d.clone(),
            None => {
                let sys = match self.files.get(&vfx.path) {
                    Some(s) => s.clone(),
                    None => {
                        let sys = ssx_assets::content::GameContent::of(root).particle_system(&vfx.path)?;
                        self.files.insert(vfx.path.clone(), sys.clone());
                        sys
                    }
                };
                let effect = sys.effect(&vfx.definition).with_context(|| format!("{}: no effect", vfx.path))?;
                let d = EffectDef::compile(effect);
                self.defs.insert(key, d.clone());
                d
            }
        };
        let behaviour = match &node.behaviour_path {
            Some(p) => self
                .behaviours
                .entry(p.clone())
                .or_insert_with(|| {
                    let data = std::fs::read(particles::fx_file(root, p)).ok()?;
                    Behaviour::parse(&data).ok().map(|b| Arc::new(Script::new(&b)))
                })
                .clone(),
            None => None,
        };
        Ok((def, behaviour))
    }
}

pub const GAMEPLAY_EFFECTS: [&str; 8] = [
    "fx_markerflare_red",
    "fx_markerflare_blue",
    "fx_markerflare_green",
    "fx_markersmoke_orange",
    "fx_markersmoke_blue",
    "fx_markersmoke_green",
    "fx_deathclouds",
    "fx_deathclouds_dark",
];

pub fn is_gameplay_effect(definition: &str) -> bool {
    GAMEPLAY_EFFECTS.iter().any(|e| e.eq_ignore_ascii_case(definition))
}

pub fn textures_of(def: &EffectDef, out: &mut Vec<String>) {
    for t in def.groups.iter().flat_map(|g| &g.actions).filter_map(|a| a.texture()) {
        if !out.iter().any(|n| n == t) {
            out.push(t.to_owned());
        }
    }
}

pub struct TrackSlot {
    pub scene_instance: u32,
    pub placement: usize,
    pub scale: Vec3,
    pub rotation: Quat,
    pub position: Vec3,
    pub placement_scale: Vec3,
    pub node: (Vec3, Quat, Vec3),
    pub ao: f32,
    pub instance: EffectInstance,
    pub behaviour: Option<Arc<Script>>,
    pub node_key: u32,
    pub effect: String,
    pub group: String,
    pub start: f32,
    pub stop: f32,
    pub instant: bool,
}

fn compose(pl: &Placement, node: &ParticleNode) -> (Vec3, Quat, Vec3) {
    let (js, jq, jp) =
        (Vec3::from_array(pl.scale), Quat::from_array(pl.rotation).normalize(), Vec3::from_array(pl.position));
    compose_parts((js, jq, jp), node_parts(node))
}

fn node_parts(node: &ParticleNode) -> (Vec3, Quat, Vec3) {
    (
        Vec4::from_array(node.scale).truncate(),
        Quat::from_array(node.orientation),
        Vec4::from_array(node.position).truncate(),
    )
}

fn compose_parts((js, jq, jp): (Vec3, Quat, Vec3), (ns, nq, np): (Vec3, Quat, Vec3)) -> (Vec3, Quat, Vec3) {
    (js * ns, jq * nq, jp + jq * (js * np))
}

#[derive(Default)]
pub struct TrackEffects {
    pub slots: Vec<TrackSlot>,
    pub offline: Option<(Box<Controllers>, OfflineHost)>,
    seen: bevy::platform::collections::HashMap<(u32, usize), u64>,
    by_entity: bevy::platform::collections::HashMap<(u32, usize), usize>,
}

impl TrackEffects {
    pub fn load(
        root: &Path,
        track: &str,
        drop: Option<u32>,
        scope: Scope,
        arena: &mut SetArena,
        defs: &mut DefCache,
        textures: &mut Vec<String>,
    ) -> Result<TrackEffects> {
        let tp = ssx_assets::track_particles::load_track_particles(root, track, drop)?;
        let slots = Self::slots_of(root, &tp, scope, arena, defs, textures)?;
        let mut out = TrackEffects { slots, ..TrackEffects::default() };
        let triggers = ssx_physics::spawn::load_trigger_volumes(root, track, drop).context("the track's entities")?;
        out.offline = Some((Box::new(Controllers::offline(&triggers)), OfflineHost::default()));
        Ok(out)
    }

    pub fn add_map(
        &mut self,
        root: &Path,
        placed: &[(u32, u32, Placement)],
        scope: Scope,
        arena: &mut SetArena,
        defs: &mut DefCache,
        textures: &mut Vec<String>,
    ) -> Result<usize> {
        let tp = ssx_assets::track_particles::map_particles(root, placed)?;
        let slots = Self::slots_of(root, &tp, scope, arena, defs, textures)?;
        let n = slots.len();
        self.slots.extend(slots);
        Ok(n)
    }

    fn slots_of(
        root: &Path,
        tp: &ssx_assets::track_particles::TrackParticles,
        scope: Scope,
        arena: &mut SetArena,
        defs: &mut DefCache,
        textures: &mut Vec<String>,
    ) -> Result<Vec<TrackSlot>> {
        let (schemas, _) = ssx_assets::avalanche_entities::batches(root, &[]).context("the schemas batch")?;
        let mut out = TrackEffects::default();
        for inst in &tp.instances {
            let o = &tp.objects[inst.object];
            let world_ao = particle_script::object_world_ao(&schemas, o.key);
            let kept =
                |vfx: &ssx_assets::particles::LynxVfx| scope == Scope::All || is_gameplay_effect(&vfx.definition);
            for node in o.lods.first().into_iter().flatten().filter(|n| n.lynx.as_ref().is_some_and(kept)) {
                let (def, behaviour) = defs.node(root, node)?;
                textures_of(&def, textures);
                let vfx = node.lynx.as_ref().expect("filtered");
                for (pi, pl) in inst.placements.iter().enumerate() {
                    let (scale, rotation, position) = compose(pl, node);
                    out.slots.push(TrackSlot {
                        scene_instance: inst.key,
                        placement: pi,
                        scale,
                        rotation,
                        position,
                        placement_scale: Vec3::from_array(pl.scale),
                        node: node_parts(node),
                        ao: if world_ao { pl.ao_multiplier() } else { 1.0 },
                        instance: EffectInstance::new(def.clone(), arena),
                        behaviour: behaviour.clone(),
                        node_key: node.key,
                        effect: vfx.definition.clone(),
                        group: node.group_name.clone(),
                        start: -1.0,
                        stop: -1.0,
                        instant: false,
                    });
                }
            }
        }
        Ok(out.slots)
    }

    pub fn switch(&mut self, clock: f32, dt: f32, session: Option<Option<&Controllers>>) {
        let entities: &[EntityController] = match session {
            Some(c) => c.map_or(&[][..], |c| &c.entities),
            None => match self.offline.as_mut() {
                Some((c, host)) => {
                    c.update_offline(dt, host);
                    &c.entities
                }
                None => &[],
            },
        };
        // PORT(choice): the entities' map is kept between updates (cleared, its memory kept) and hashed with Bevy's
        // fixed hasher (foldhash); it was a new `std` map (SipHash) of every entity each update (the whole switch
        // 0.10 ms of the main thread a ride step on Mount Eddie, 0.13 on Vorassay: a traced build with timers, round
        // perf-x1). It holds the same entries (the last entity of a key), and only its lookups and the order-free
        // `seen` writes read it. The console's controllers' storage is not decoded.
        let by_entity = &mut self.by_entity;
        by_entity.clear();
        for (i, e) in entities.iter().enumerate() {
            if e.owner.is_none() {
                by_entity.insert((e.instance, e.placement), i);
            }
        }
        for s in &mut self.slots {
            let key = (s.scene_instance, s.placement);
            let Some(e) = by_entity.get(&key).map(|&i| &entities[i]) else { continue };
            if e.body.is_some() {
                (s.scale, s.rotation, s.position) = compose_parts((s.placement_scale, e.rotation, e.position), s.node);
            }
            let seen = self.seen.get(&key).copied().unwrap_or(0);
            for op in e.fx.particles.iter().filter(|o| o.seq > seen) {
                let hit = match &op.target {
                    ParticleTarget::Track(Target::Node(n)) => {
                        n.is_empty() || ssx_assets::vault_db::fnv1(n) == s.node_key
                    }
                    ParticleTarget::Track(Target::Effect(x)) => *x == s.effect,
                    ParticleTarget::Track(Target::Group(g)) => *g == s.group,
                    ParticleTarget::GearItem(_) => false,
                };
                if hit {
                    match op.switch {
                        Switch::Activate => (s.start, s.stop, s.instant) = (clock, -1.0, false),
                        Switch::Deactivate => (s.start, s.stop, s.instant) = (-1.0, clock, false),
                        Switch::DeactivateImmediately => (s.start, s.stop, s.instant) = (-1.0, clock, true),
                        Switch::None => {}
                    }
                }
            }
        }
        for (&key, &i) in by_entity.iter() {
            if let Some(op) = entities[i].fx.particles.last() {
                self.seen.insert(key, op.seq);
            }
        }
    }

    pub fn controlled(&self) -> usize {
        let Some((c, _)) = self.offline.as_ref() else { return 0 };
        self.slots.iter().filter(|s| c.entity(s.scene_instance, s.placement).is_some()).count()
    }

    pub fn record(&self, i: usize, blend: Vec<(u8, f32)>) -> RecordView {
        let s = &self.slots[i];
        RecordView {
            scale: s.scale,
            rotation: s.rotation,
            position: s.position,
            start: s.start,
            stop: s.stop,
            instant_off: s.instant,
            ao: s.ao,
            blend,
        }
    }
}

pub fn visibility(
    centre: Vec3,
    scale: Vec3,
    radius: f32,
    clip_from_world: &Mat4,
    camera: Vec3,
    tan_half_fov: f32,
) -> Visibility {
    let half = 2.0 * radius * scale.max_element();
    let r = (Vec3::splat(half)).length();
    let m = clip_from_world.transpose();
    let (r0, r1, r2, r3) = (m.x_axis, m.y_axis, m.z_axis, m.w_axis);
    let planes = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 - r2];
    let inside = planes.iter().all(|p| {
        let n = p.truncate();
        (n.dot(centre) + p.w) / n.length() >= -r
    });
    let d = (centre - camera).length();
    let metric = if d > 0.0 { r / (d * tan_half_fov) } else { f32::MAX };
    Visibility { metric, visible: inside && metric > 0.002 }
}
