use std::sync::Arc;

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use ssx_assets::particles::ParticleNode;

use super::load::{load_heli_effects, load_new_textures, load_rider_effects};
use super::snapshot::Records;
use super::{ParticleFrame, ParticleLight, ParticleState, ParticleWorld, SUN_DISTANCE, Spray, rider, shadow_query};
use crate::particles::render::FrameDraws;
use crate::particles::sim::{Context, Draw, DrawBuffers, EffectInstance, RecordView, Visibility};
use crate::particles::spray::{EffectRecord, SprayInputs};
use crate::render::lighting::game::GameLightUniform;
use crate::render::post::GamePostCamera;
use crate::ride::Ride;
use crate::rider::anim::JointRecord;
use crate::rider::skin::RiderRig;

fn newest_joints(rig: &RiderRig, ride: &Ride) -> Vec<JointRecord> {
    let (pose, graph, _) = rig.anim.posed();
    crate::rider::anim::world_records(graph, &pose.nodes, ride.step().0.traj)
}

fn compose(joint: &GlobalTransform, node: &ParticleNode) -> (Vec3, Quat, Vec3) {
    let (js, jq, jp) = joint.to_scale_rotation_translation();
    rider::compose(&JointRecord { s: js, q: jq, p: jp }, node)
}

fn spray_records(
    spray: &mut Spray,
    ride: &Ride,
    clock: f32,
    board: &JointRecord,
    precipitation: f32,
) -> (Vec<EffectRecord>, SprayInputs) {
    spray.inputs.step(ride);
    let inputs = spray.inputs.inputs(ride);
    let records = driver_records(spray, &inputs, clock, board, precipitation);
    (records, inputs)
}

fn driver_records(
    spray: &mut Spray,
    inputs: &SprayInputs,
    clock: f32,
    board: &JointRecord,
    precipitation: f32,
) -> Vec<EffectRecord> {
    let inputs = *inputs;
    let present = spray.table.map(|row| row.map(|e| e.is_some()));
    for (s, k, on) in spray.driver.update(&inputs, &present) {
        let Some(e) = spray.table[s][k] else { continue };
        let slot = &mut spray.effects[e];
        (slot.start, slot.stop, slot.instant) = if on { (clock, -1.0, false) } else { (-1.0, clock, false) };
    }
    let mut records: Vec<EffectRecord> = spray
        .effects
        .iter()
        .map(|e| {
            let (scale, rotation, position) = rider::compose(board, &e.node);
            let blend = crate::particles::inputs::default_blend(e.behaviour.as_deref(), &inputs, precipitation);
            EffectRecord { scale, rotation, position, ao: inputs.ao, blend }
        })
        .collect();
    for (s, row) in spray.table.iter().enumerate() {
        for (k, e) in row.iter().enumerate() {
            if let Some(e) = e
                && spray.driver.is_on(s, k)
            {
                spray.driver.apply(k, &mut records[*e]);
            }
        }
    }
    records
}

fn bot_spray_inputs(pose: &ssx_assets::ghosts::GhostPose) -> SprayInputs {
    use ssx_formats::ghost::{FLAG_GROUND_RAY, FLAG_IN_AIR, FLAG_IS_AWESOME};
    SprayInputs {
        is_awesome: pose.flags & FLAG_IS_AWESOME != 0,
        velocity: Vec3::from_array(pose.velocity),
        spray_velocity: Vec3::X * pose.snow_spray_speed,
        snow_spray: Vec3::from_array(pose.snow_spray_carving),
        landing_impact: pose.landing_impact,
        ground_contact: pose.flags & FLAG_GROUND_RAY != 0,
        in_air: pose.flags & FLAG_IN_AIR != 0,
        snow_coverage: pose.snow_coverage,
        // TODO(guess): the bot's AO component holds 1.0, its constructor's value (0x8294FD68): its `AoGroundContactTrack` needs interface 0x57EF8508 (and `WorldAo` 0xC5AB0A1E), creator 0x82928AB0, and two component classes answer 0x57EF8508 (QueryInterface 0x82AC0440, 0x82ACA240); settle: that interface's owner for a `ghost_<character>` animatable.
        ao: 1.0,
        ..SprayInputs::default()
    }
}

fn spray_views(spray: &Spray, records: Vec<EffectRecord>) -> Vec<RecordView> {
    spray
        .effects
        .iter()
        .zip(records)
        .map(|(e, r)| RecordView {
            scale: r.scale,
            rotation: r.rotation,
            position: r.position,
            start: e.start,
            stop: e.stop,
            instant_off: e.instant,
            ao: r.ao,
            blend: r.blend,
        })
        .collect()
}

fn live_spray_records(
    w: &mut ParticleState,
    bots: Option<&crate::ride::bots::Bots>,
    root: Option<&crate::render::lighting::BigRoot>,
    clock: f32,
    precipitation: f32,
) -> Vec<Vec<RecordView>> {
    let (Some(bots), Some(root)) = (bots, root) else { return Vec::new() };
    if w.live_sprays.pool.is_empty() {
        return Vec::new();
    }
    let before = w.texture_names.len();
    let ParticleState { live_sprays, arena, defs, texture_names, .. } = w;
    let build = |set: &ssx_assets::particles::SnowSpraySet, arena: &mut crate::particles::sim::SetArena| {
        super::load::spray_of(set, &root.0, arena, defs, texture_names, true)
    };
    match live_sprays.assign(bots.live.sprays(), arena, build, bot_spray_inputs) {
        Ok(true) => load_new_textures(w, &root.0, before),
        Ok(false) => {}
        Err(e) => {
            warn!("particles: a live racer's lod snow spray unavailable: {e:#}");
            w.live_sprays.set = None;
        }
    }
    let mut out = Vec::with_capacity(w.live_sprays.pool.len());
    for s in &mut w.live_sprays.pool {
        let Some(board) = s.board else {
            out.push(Vec::new());
            continue;
        };
        let records = driver_records(&mut s.spray, &s.inputs, clock, &board, precipitation);
        out.push(spray_views(&s.spray, records));
    }
    out
}

// PORT(choice): with a ride the manager update runs once per frame after the frame's steps, over their summed length,
// and its snapshot is stamped with the newest step; the console runs it after every step (0x823D18B8), a snapshot each.
// The records read what the port steps and draws in the frame (the avalanche's chunks, the racers, the helicopter), so
// a frame of two or more steps (below 30 fps, or after a stall) draws between the update before them and its own.
fn manager_dt(w: &mut ParticleState, ride: Option<&Ride>, real_dt: f32) -> Option<f32> {
    match ride {
        Some(ride) => {
            if ride.steps == w.last_step {
                return None;
            }
            if ride.steps < w.last_step {
                w.last_step = 0;
            }
            let dt = (ride.steps - w.last_step) as f32 * ride.step().1 / 60.0;
            w.last_step = ride.steps;
            Some(dt)
        }
        None => {
            w.pending += real_dt;
            let n = (w.pending * 30.0).floor();
            if n < 1.0 {
                return None;
            }
            w.pending -= n / 30.0;
            Some(n.min(3.0) / 30.0)
        }
    }
}

pub(super) struct Done {
    draws: FrameDraws,
    light: GameLightUniform,
}

#[derive(Clone, Copy)]
struct View {
    clip_from_world: Mat4,
    view_from_world: Mat4,
    position: Vec3,
    forward: Vec3,
    tan_half_fov: f32,
}

impl View {
    fn visibility(&self, rec: &RecordView, inst: &EffectInstance) -> Visibility {
        let (radius, _) = inst.cull_params();
        crate::particles::track::visibility(
            rec.position,
            rec.scale,
            radius,
            &self.clip_from_world,
            self.position,
            self.tan_half_fov,
        )
    }
}

#[derive(Clone, Copy)]
struct Sources<'a> {
    ride: Option<&'a Ride>,
    rig: Option<&'a RiderRig>,
    bots: Option<&'a crate::ride::bots::Bots>,
    heli: Option<&'a crate::ride::helicopter::HeliVisual>,
    avalanche: Option<&'a crate::render::avalanche::AvalancheScene>,
    big_root: Option<&'a crate::render::lighting::BigRoot>,
    camera: Vec3,
}

fn manager_update(
    w: &mut ParticleState,
    src: Sources,
    geotag_bursts: Option<ResMut<crate::render::geotags::GeotagBursts>>,
    dt: f32,
) {
    let Sources { ride, rig, bots: bot_boards, heli, avalanche, big_root, camera: cam_pos } = src;
    w.clock += dt;
    let clock = w.clock;
    if let (Some(mut queued), Some(root)) = (geotag_bursts, big_root) {
        let pending = std::mem::take(&mut queued.pending);
        if !pending.is_empty() {
            let before = w.texture_names.len();
            for line in w.geotags.start(pending, &root.0, &mut w.defs, &mut w.texture_names, &mut w.arena, clock) {
                info!("particles: {line}");
            }
            load_new_textures(w, &root.0, before);
        }
    }
    if let Some(c) = ride.and_then(|r| r.session.controllers.as_ref()) {
        w.weather.follow(&c.weather);
    }
    w.weather.update(dt);
    let mut weather = w.weather.weather();
    if let Some(p) = w.switches.precipitation {
        weather.precipitation = p;
    }
    let rider_rig = ride.zip(rig);
    let joints = rider_rig.map(|(ride, rig)| newest_joints(rig, ride));
    let board = rig.and_then(|r| r.anim.posed().1.find("Board"));
    let mut spray_inputs = None;
    let mut spray_recs: Vec<RecordView> = Vec::new();
    if let (Some(ride), Some(spray), Some(board)) =
        (ride, w.spray.as_mut(), joints.as_ref().zip(board).and_then(|(j, b)| j.get(b)))
    {
        let (records, inputs) = spray_records(spray, ride, clock, board, weather.precipitation);
        spray_recs = spray
            .effects
            .iter()
            .zip(records)
            .map(|(e, r)| RecordView {
                scale: r.scale,
                rotation: r.rotation,
                position: r.position,
                start: e.start,
                stop: e.stop,
                instant_off: e.instant,
                ao: r.ao,
                blend: r.blend,
            })
            .collect();
        spray_inputs = Some(inputs);
    }
    let mut bot_recs: Vec<Vec<RecordView>> = Vec::new();
    if let (Some(race), Some(drawn)) = (ride.and_then(|r| r.session.bots.as_ref()), bot_boards) {
        for (k, spray) in w.bot_sprays.iter_mut().enumerate() {
            let (Some(b), Some(Some(board))) = (race.bots.get(k), drawn.boards.get(k)) else {
                bot_recs.push(Vec::new());
                continue;
            };
            let inputs = if race.running && !b.removed {
                bot_spray_inputs(&b.replay.pose(b.clock.seconds))
            } else {
                SprayInputs { ao: 1.0, ..SprayInputs::default() }
            };
            let records = driver_records(spray, &inputs, clock, board, weather.precipitation);
            bot_recs.push(
                spray
                    .effects
                    .iter()
                    .zip(records)
                    .map(|(e, r)| RecordView {
                        scale: r.scale,
                        rotation: r.rotation,
                        position: r.position,
                        start: e.start,
                        stop: e.stop,
                        instant_off: e.instant,
                        ao: r.ao,
                        blend: r.blend,
                    })
                    .collect(),
            );
        }
    }
    let live_recs = live_spray_records(w, bot_boards, big_root, clock, weather.precipitation);
    let mut rider_recs: Vec<Option<RecordView>> = Vec::new();
    if let (Some(ride), Some(rider), Some(inputs), Some(joints)) =
        (ride, w.rider.as_mut(), spray_inputs.as_ref(), joints.as_ref())
    {
        let mut fx_inputs = *inputs;
        if let Some(sup) = w.switches.tricky {
            (fx_inputs.is_awesome, fx_inputs.is_super_awesome) = (true, sup);
        }
        let inputs = &fx_inputs;
        let log = w.switches.log_is("on") || w.switches.log_is("rider");
        let ops = ride.session.controllers.as_ref().map_or(&[][..], |c| c.fx.particles.as_slice());
        let switched = rider.switch(clock, ops, inputs);
        if !switched.is_empty() && log {
            info!(
                "particles: step {} ({:.2} s, rider at {:.1}): {}",
                ride.steps,
                ride.seconds(),
                ride.state.traj,
                switched.join(", ")
            );
        }
        rider_recs = rider.records(joints, inputs, weather.precipitation);
    }
    let mut heli_recs: Vec<Option<RecordView>> = Vec::new();
    if let (Some(ride), Some(h), Some(fx)) = (ride, heli, w.heli.as_mut()) {
        let entity = crate::ride::helicopter::drop_heli(ride);
        let inputs = SprayInputs {
            ao: entity.map_or(1.0, |e| e.world_ao),
            downwash_visual_strength: h.downwash.map_or(0.0, |d| d.visual_strength),
            ..SprayInputs::default()
        };
        let switched = fx.switch(clock, entity.map_or(&[][..], |e| e.particles.as_slice()), &inputs);
        if !switched.is_empty() && w.switches.log_is("heli") {
            info!("particles: step {} ({:.2} s), helicopter: {}", ride.steps, ride.seconds(), switched.join(", "));
        }
        heli_recs = fx.records(&h.newest, &inputs, weather.precipitation);
    }
    w.track.switch(clock, dt, ride.map(|r| r.session.controllers.as_ref()));
    let track_inputs = spray_inputs.unwrap_or_default();
    let track_recs: Vec<RecordView> = (0..w.track.slots.len())
        .map(|i| {
            let blend = crate::particles::inputs::default_blend(
                w.track.slots[i].behaviour.as_deref(),
                &track_inputs,
                weather.precipitation,
            );
            w.track.record(i, blend)
        })
        .collect();
    let owner_ao = track_inputs.ao.max(0.0);
    let trail_recs: Vec<RecordView> = match avalanche {
        Some(scene) => w
            .trails
            .iter_mut()
            .map(|t| {
                let c = &scene.sets[t.set].chunks[t.chunk];
                let active = t.start != -1.0 && t.stop == -1.0;
                if c.curr.shown {
                    if active {
                        t.blend = vec![(1, 1.0 - c.weight), (2, c.weight)];
                    } else {
                        (t.start, t.stop, t.blend) = (clock, -1.0, vec![(1, 1.0), (2, 0.0)]);
                    }
                } else if active {
                    (t.start, t.stop) = (-1.0, clock);
                }
                let entity = GlobalTransform::from(Transform {
                    translation: c.curr.position,
                    rotation: c.curr.rotation,
                    scale: Vec3::splat(c.curr.scale),
                });
                let (scale, rotation, position) = compose(&entity, &t.node);
                RecordView {
                    scale,
                    rotation,
                    position,
                    start: t.start,
                    stop: t.stop,
                    instant_off: false,
                    ao: owner_ao,
                    blend: t.blend.clone(),
                }
            })
            .collect(),
        None => Vec::new(),
    };
    let geotag_blend = crate::particles::inputs::default_blend(None, &track_inputs, weather.precipitation);
    let geotag_recs = w.geotags.records(ride.and_then(|r| r.session.geotags.as_ref()), &geotag_blend);
    let tombstone_recs = w.tombstones.records(ride.and_then(|r| r.session.tombstones.as_ref()), &geotag_blend);
    let sun_rec = w.sun.as_ref().map(|sun| {
        let locator = JointRecord { s: Vec3::ONE, q: Quat::IDENTITY, p: cam_pos + sun.dir * SUN_DISTANCE };
        let (scale, rotation, position) = rider::compose(&locator, &sun.node);
        let blend = crate::particles::inputs::default_blend(None, &track_inputs, weather.precipitation);
        RecordView { scale, rotation, position, start: 0.0, stop: -1.0, instant_off: false, ao: 1.0, blend }
    });
    // PORT(choice): without a ride (fly camera) there is no focus character; the camera stands in (viewer-only); the
    // game always rides a character.
    let point = ride.map_or(cam_pos, |r| r.state.traj);
    let records = Records {
        trails: trail_recs,
        spray: spray_recs,
        bots: bot_recs,
        live: live_recs,
        rider: rider_recs,
        heli: heli_recs,
        track: track_recs,
        sun: sun_rec,
        geotags: geotag_recs,
        tombstones: tombstone_recs,
    };
    w.snapshots.push(&records, clock, point, weather, ride.map_or(0, |r| r.steps));
}

fn frame_locators(w: &ParticleState, records: &mut Records, src: Sources) {
    if let Some(drawn) = src.bots {
        for ((spray, recs), board) in w.bot_sprays.iter().zip(&mut records.bots).zip(&drawn.boards) {
            if let Some(board) = board {
                relocate(spray, recs, board);
            }
        }
        for (s, recs) in w.live_sprays.pool.iter().zip(&mut records.live) {
            if let Some((_, board, _)) = s.rider.and_then(|id| drawn.live.sprays().find(|r| r.0 == id)) {
                relocate(&s.spray, recs, &board);
            }
        }
    }
    if let (Some(sun), Some(rec)) = (&w.sun, records.sun.as_mut()) {
        let locator = JointRecord { s: Vec3::ONE, q: Quat::IDENTITY, p: src.camera + sun.dir * SUN_DISTANCE };
        (rec.scale, rec.rotation, rec.position) = rider::compose(&locator, &sun.node);
    }
}

fn relocate(spray: &Spray, recs: &mut [RecordView], board: &JointRecord) {
    let mut located: Vec<EffectRecord> = spray
        .effects
        .iter()
        .map(|e| {
            let (scale, rotation, position) = rider::compose(board, &e.node);
            EffectRecord { scale, rotation, position, ao: 0.0, blend: Vec::new() }
        })
        .collect();
    for (s, row) in spray.table.iter().enumerate() {
        for (k, e) in row.iter().enumerate() {
            if let Some(e) = e
                && spray.driver.is_on(s, k)
                && let Some(r) = located.get_mut(*e)
            {
                spray.driver.apply(k, r);
            }
        }
    }
    for (rec, l) in recs.iter_mut().zip(located) {
        (rec.scale, rec.rotation, rec.position) = (l.scale, l.rotation, l.position);
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update(
    ride: Option<Res<Ride>>,
    time: Res<Time>,
    world: Option<ResMut<ParticleWorld>>,
    rig: Option<Res<RiderRig>>,
    big_root: Option<Res<crate::render::lighting::BigRoot>>,
    cameras: Query<(&GlobalTransform, &Projection), With<GamePostCamera>>,
    lighting: Option<Res<crate::render::lighting::GameLighting>>,
    avalanche: Option<Res<crate::render::avalanche::AvalancheScene>>,
    shadow: Option<Res<shadow_query::PlayerShadowQuery>>,
    bot_boards: Option<Res<crate::ride::bots::Bots>>,
    geotag_bursts: Option<ResMut<crate::render::geotags::GeotagBursts>>,
    heli: Option<Res<crate::ride::helicopter::HeliVisual>>,
) {
    let Some(mut world) = world else { return };
    let world = &mut world.0;
    let Some(w) = world.settle() else { return };
    let Ok((camera, projection)) = cameras.single() else { return };
    let ride = ride.as_deref();
    // PORT(choice): the rider's effects and their textures load in the first frame with the rider, under the loading
    // screen; in the ride's first update they were part of a 64 ms particles update on the game thread when the
    // loading screen went (alps0101, development laptop, `trace_chrome`). They read the rig's names and the loadout
    // alone, and nothing else enters the effects' arena before them; the console's loading is not decoded.
    if let (Some(ride), Some(rig), Some(root), Some(l), false) =
        (ride, rig.as_deref(), &big_root, &lighting, w.rider_tried)
    {
        load_rider_effects(w, &root.0, l.disc_track(), rig, ride);
    }
    // PORT(choice): the helicopter's effects and their textures load in the first frame with the helicopter, after the
    // rider's, under the loading screen; in the ride's first update they took 57 ms of the game thread when the
    // loading screen went (`exp_siberia_boris_drop_4_trick`, a 111 ms first ride frame, development laptop,
    // `trace_chrome`). They read the helicopter's rig and particle nodes alone; the draws keep their order (the
    // effects' fixed order, then the depth sort, not the arena's sets); the console's loading is not decoded.
    if let (Some(h), Some(root), false) = (heli.as_deref(), &big_root, w.heli_tried) {
        load_heli_effects(w, &root.0, h);
    }
    let clip_from_view = projection.get_clip_from_view();
    let view_from_world = camera.to_matrix().inverse();
    let view = View {
        clip_from_world: clip_from_view * view_from_world,
        view_from_world,
        position: camera.translation(),
        forward: camera.forward().as_vec3(),
        tan_half_fov: (1.0 / clip_from_view.x_axis.x).min(1.0 / clip_from_view.y_axis.y),
    };
    let src = Sources {
        ride,
        rig: rig.as_deref(),
        bots: bot_boards.as_deref(),
        heli: heli.as_deref(),
        avalanche: avalanche.as_deref(),
        big_root: big_root.as_deref(),
        camera: view.position,
    };
    let updated = match manager_dt(w, ride, time.delta_secs()) {
        Some(dt) => {
            manager_update(w, src, geotag_bursts, dt);
            true
        }
        None => false,
    };
    // PORT(choice): above the console's 60 Hz (`LoopRender` 0x8236DF08, presented every vertical blank) the step runs
    // every drawn frame as well, so the trails and the local-space effects stay on the joints drawn in each frame; the
    // emission counts follow the update rate as the game's count makes them (`Lynx_EmitBase_GetEmitCount` 0x82BAA688:
    // `fx_trickyflare_ribbon`'s rate 100 emits 100 a second at 30 Hz, 90 at 60 Hz, 144 at 144 Hz) and every frame pays
    // the step (0.56 ms a call on a compute thread on Mount Eddie, round perf-x1). The console never drew above 60.
    if ride.is_none() && !updated {
        return;
    }
    let Some(mut frame) = w.snapshots.frame(ride.map(Ride::render_steps)) else { return };
    frame_locators(w, &mut frame.records, src);
    let controllers = match ride {
        Some(r) => r.session.controllers.as_ref(),
        None => w.track.offline.as_ref().map(|(c, _)| &**c),
    };
    let global_light = w.light.as_mut().map_or_else(Default::default, |l| {
        l.pick(frame.point, &view.clip_from_world, view.position, view.tan_half_fov, controllers)
    });
    let light = lighting.map_or_else(GameLightUniform::viewer_default, |l| GameLightUniform::from_preset(&l));
    let player_shadow = crate::particles::inputs::player_shadow(shadow.as_deref());
    let (records, clock, weather) = (frame.records, frame.clock, frame.weather);
    // PORT(choice): the records' Lynx step (the instances' apply and update, the draws, their sort and the parameter
    // registry: everything that reads no other system's data) runs as a task of Bevy's compute pool beside the frame's
    // other systems, and `finish` joins it in `Last`, before the frame's extract: the same work in the same order on
    // the same state, so the same draws. Run in the system it held PostUpdate's later systems and command syncs
    // (visibility, the shadows' culling) behind it; the console's render object stepped its particles in its render
    // pass (0x82964E6C).
    world.spawn(move |w| {
        let draws = step(w, &records, &view, clock, player_shadow, weather, &global_light);
        Done { draws, light }
    });
}

// PORT(choice): it runs on the main thread (`NonSendMarker`): a compute thread blocked on the step could be the one
// the step waits for; the console had no task pool.
pub(super) fn finish(
    _main_thread: NonSendMarker,
    world: Option<ResMut<ParticleWorld>>,
    mut frame_out: ResMut<ParticleFrame>,
) {
    let Some(mut world) = world else { return };
    if !world.0.pending() {
        return;
    }
    let world = &mut world.0;
    let Some(textures) = world.settle().map(|w| w.textures.clone()) else { return };
    let Some(done) = world.take_done() else { return };
    *frame_out =
        ParticleFrame { draws: Arc::new(done.draws), textures, light: done.light, serial: frame_out.serial + 1 };
}

fn step(
    w: &mut ParticleState,
    rec: &Records,
    view: &View,
    clock: f32,
    player_shadow: f32,
    weather: crate::particles::sim::Weather,
    global_light: &ParticleLight,
) -> FrameDraws {
    let _span = bevy::log::info_span!("particles step").entered();
    let cam_pos = view.position;
    let lynx_dt = (clock - w.last_render_time).clamp(-0.1, 0.1);
    let frame_dt = clock - w.last_render_time;
    let arena = &mut w.arena;
    for (t, rec) in w.trails.iter_mut().zip(&rec.trails) {
        let v = view.visibility(rec, &t.instance);
        t.instance.apply_record(rec, clock, frame_dt, v, arena);
    }
    if let Some(spray) = w.spray.as_mut() {
        for (e, rec) in spray.effects.iter_mut().zip(&rec.spray) {
            let v = view.visibility(rec, &e.instance);
            e.instance.apply_record(rec, clock, frame_dt, v, arena);
        }
    }
    for (spray, recs) in w.bot_sprays.iter_mut().zip(&rec.bots) {
        for (e, rec) in spray.effects.iter_mut().zip(recs) {
            let v = view.visibility(rec, &e.instance);
            e.instance.apply_record(rec, clock, frame_dt, v, arena);
        }
    }
    for (s, recs) in w.live_sprays.pool.iter_mut().zip(&rec.live) {
        for (e, rec) in s.spray.effects.iter_mut().zip(recs) {
            let v = view.visibility(rec, &e.instance);
            e.instance.apply_record(rec, clock, frame_dt, v, arena);
        }
    }
    if let Some(rider) = w.rider.as_mut() {
        for (e, rec) in rider.slots.iter_mut().zip(&rec.rider) {
            if let Some(rec) = rec {
                let v = view.visibility(rec, &e.instance);
                e.instance.apply_record(rec, clock, frame_dt, v, arena);
            }
        }
    }
    if let Some(heli) = w.heli.as_mut() {
        for (e, rec) in heli.slots.iter_mut().zip(&rec.heli) {
            if let Some(rec) = rec {
                let v = view.visibility(rec, &e.instance);
                e.instance.apply_record(rec, clock, frame_dt, v, arena);
            }
        }
    }
    for (s, rec) in w.track.slots.iter_mut().zip(&rec.track) {
        let v = view.visibility(rec, &s.instance);
        s.instance.apply_record(rec, clock, frame_dt, v, arena);
    }
    if let (Some(sun), Some(rec)) = (w.sun.as_mut(), &rec.sun) {
        let v = view.visibility(rec, &sun.instance);
        sun.instance.apply_record(rec, clock, frame_dt, v, arena);
    }
    for (s, rec) in w.geotags.slots.iter_mut().zip(&rec.geotags) {
        let v = view.visibility(rec, &s.instance);
        s.instance.apply_record(rec, clock, frame_dt, v, arena);
    }
    for (inst, rec) in w.tombstones.slots.iter_mut().zip(&rec.tombstones) {
        let v = view.visibility(rec, inst);
        inst.apply_record(rec, clock, frame_dt, v, arena);
    }
    w.last_render_time = clock;
    w.frame = w.frame.wrapping_add(1);
    if lynx_dt > 0.0 {
        w.dt_smoothed = 0.5 * w.dt_smoothed + 0.5 * lynx_dt;
    }
    let ctx = Context {
        time_ms: (clock * 1000.0) as i32,
        dt: lynx_dt,
        frame: w.frame,
        dt_smoothed: w.dt_smoothed,
        camera: cam_pos,
        player_shadow,
        weather,
    };
    let mut instances: Vec<&mut EffectInstance> = Vec::new();
    if let Some(spray) = w.spray.as_mut() {
        instances.extend(spray.effects.iter_mut().map(|e| &mut e.instance));
    }
    for spray in &mut w.bot_sprays {
        instances.extend(spray.effects.iter_mut().map(|e| &mut e.instance));
    }
    for s in &mut w.live_sprays.pool {
        instances.extend(s.spray.effects.iter_mut().map(|e| &mut e.instance));
    }
    if let Some(rider) = w.rider.as_mut() {
        instances.extend(rider.slots.iter_mut().map(|e| &mut e.instance));
    }
    if let Some(heli) = w.heli.as_mut() {
        instances.extend(heli.slots.iter_mut().map(|e| &mut e.instance));
    }
    instances.extend(w.track.slots.iter_mut().map(|s| &mut s.instance));
    instances.extend(w.trails.iter_mut().map(|t| &mut t.instance));
    instances.extend(w.sun.as_mut().map(|s| &mut s.instance));
    instances.extend(w.geotags.slots.iter_mut().map(|s| &mut s.instance));
    if !rec.tombstones.is_empty() {
        instances.extend(w.tombstones.slots.iter_mut());
    }
    for inst in instances.iter_mut() {
        inst.update(&ctx, &mut w.rng, arena);
    }
    let mut bufs = DrawBuffers::default();
    let hide = &w.switches.hide;
    for (i, inst) in instances.iter().enumerate() {
        if hide.iter().any(|n| *n == inst.def.name) {
            continue;
        }
        inst.draws(i, &ctx, view.forward, arena, &mut bufs);
    }
    drop(instances);
    sort_draws(&mut bufs.draws, &view.view_from_world);
    let draws = &bufs.draws;
    if w.switches.log_is("on") && w.frame.is_multiple_of(15) {
        let mut per: std::collections::BTreeMap<&str, (usize, usize, std::collections::BTreeSet<usize>)> =
            Default::default();
        for s in &w.track.slots {
            let e = per.entry(s.instance.def.name.as_str()).or_default();
            e.0 += (s.instance.state == 0) as usize;
            e.1 += 1;
            e.2.extend(s.instance.sets.iter().copied());
        }
        let parts: Vec<String> = per
            .iter()
            .map(|(n, (r, t, sets))| {
                let p: usize = sets.iter().map(|&i| w.arena.sets[i].particles.len()).sum();
                format!("{n} {r}/{t} {p}")
            })
            .collect();
        let bots: Vec<String> = w
            .bot_sprays
            .iter()
            .map(|sp| {
                let on: Vec<String> = sp
                    .effects
                    .iter()
                    .filter(|e| e.start >= 0.0 && e.stop < 0.0)
                    .map(|e| e.instance.def.name.clone())
                    .collect();
                let p: usize = sp
                    .effects
                    .iter()
                    .flat_map(|e| e.instance.sets.iter())
                    .map(|&i| w.arena.sets[i].particles.len())
                    .sum();
                format!("[{}] {p}", on.join(" "))
            })
            .collect();
        info!(
            "particles t {clock:.2}: {} draws, precipitation {:.3}, wind {:.2} | {} | rider: {}{}{}",
            draws.len(),
            weather.precipitation,
            weather.wind,
            parts.join(", "),
            w.rider.as_ref().map_or_else(String::new, |r| r.summary(&w.arena)),
            w.heli.as_ref().map_or_else(String::new, |h| format!(" | helicopter: {}", h.summary(&w.arena))),
            if bots.is_empty() { String::new() } else { format!(" | bots: {}", bots.join(", ")) }
        );
        for d in draws {
            debug!(
                "  draw {} {}/{} half {} tonemap {} n {} aabb {:.1?}..{:.1?} age {} t {}",
                d.effect,
                d.render.shader,
                d.render.technique,
                d.half,
                d.tonemap,
                d.records.len(),
                d.aabb.0,
                d.aabb.1,
                d.age_max,
                d.time_now
            );
        }
    }
    crate::particles::render::frame_draws(&bufs, &mut w.registry, global_light, &w.textures, &mut w.unported)
}

fn sort_draws(draws: &mut Vec<Draw>, view_from_world: &Mat4) {
    let mut keyed: Vec<(i32, Draw)> = Vec::with_capacity(draws.len());
    let all = std::mem::take(draws);
    let mut iter = all.into_iter().peekable();
    while let Some(first) = iter.next() {
        let effect = first.effect;
        let mut run = vec![first];
        while iter.peek().is_some_and(|d| d.effect == effect) {
            run.push(iter.next().unwrap());
        }
        let (lo, hi) = run
            .iter()
            .fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), d| (a.min(d.aabb.0), b.max(d.aabb.1)));
        let z = view_from_world.transform_point3((lo + hi) * 0.5).z;
        let d = (0.0005 * (200.0 - z)).clamp(0.0, 1.0);
        let key24 = ((1.0 - d) * 16_777_215.0) as i64 as u32 & 0x00FF_FFFF;
        for (j, draw) in run.into_iter().enumerate() {
            let key = ((draw.layer as u32 & 0xF) << 28) | (key24 << 4) | (j as u32 & 0xF);
            keyed.push((key as i32, draw));
        }
    }
    keyed.sort_by_key(|(k, _)| *k);
    draws.extend(keyed.into_iter().map(|(_, d)| d));
}
