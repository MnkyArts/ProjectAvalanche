use ssx_assets::music_script::GameMusicState;
use ssx_assets::sfx_avalanche::AvalancheMsg;
use ssx_assets::sfx_dsp::sound3d::{BOARD_CENTER, Emitter};
use ssx_assets::sfx_game::{RideAudio, SurfaceWeights, Vignettes};
use ssx_physics::Quat;
use ssx_physics::rider::{RiderState, World};
use ssx_physics::session::Session;
use ssx_physics::surface::ROCK;

#[derive(Default)]
pub struct InputTrack {
    ride_state: Option<ssx_physics::rider::RideState>,
    tricky_len: f32,
    was_awesome: bool,
    dead_by_death_plane: bool,
    zones_inside: [bool; 3],
    tunnel: (bool, bool),
    advise_inside: [bool; ADVISE_ZONES.len()],
    advised: Option<Advised>,
    wingsuit_deployed: bool,
    air_ms: i32,
    land_ms: i32,
}

impl InputTrack {
    fn air_counter(&mut self, in_air: bool, dt_ms: i32) -> i32 {
        if !in_air {
            self.air_ms = 0;
        } else if ((self.air_ms as u32) & 0xFFFF) < (0xFFFF_i32.wrapping_sub(dt_ms)) as u32 {
            self.air_ms = self.air_ms.wrapping_add(dt_ms);
        }
        self.air_ms
    }

    fn land_counter(&mut self, ticks: f32, dt_ms: i32, wingsuit: bool, in_air: bool) -> i32 {
        if wingsuit {
            if self.land_ms > dt_ms {
                self.land_ms -= dt_ms / 3;
            }
        } else if !in_air {
            self.land_ms = if self.land_ms >= dt_ms { self.land_ms - dt_ms } else { 0 };
        } else {
            let estimate = fctiwz(ticks * f32::from_bits(0x4205_5555));
            let x = (self.land_ms as f32)
                .mul_add(f32::from_bits(0x3F66_6666), estimate as f32 * f32::from_bits(0x3DCC_CCCD));
            let x = if -x >= 0.0 { 0.0 } else { x };
            let x = if 65525.0 - x >= 0.0 { x } else { 65525.0 };
            self.land_ms = fctiwz(x);
        }
        self.land_ms
    }
}

fn fctiwz(x: f32) -> i32 {
    if x.is_nan() { i32::MIN } else { x as i32 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advised {
    Wingsuit,
    Ridge,
    JumpDeath,
    HeliGrab,
}

pub const ADVISE_ZONES: [(&str, Advised); 7] = [
    ("audupwingsuit", Advised::Wingsuit),
    ("audinjumpdeath", Advised::JumpDeath),
    ("audinheligrab", Advised::HeliGrab),
    ("audinridgeboth", Advised::Ridge),
    ("audinridgeleft", Advised::Ridge),
    ("audinridgeright", Advised::Ridge),
    ("audinwingsuit", Advised::Wingsuit),
];

pub fn advise_zones(session: &Session, world: &World) -> [bool; ADVISE_ZONES.len()] {
    ADVISE_ZONES.map(|(n, _)| session.tracker.touching_name(&world.triggers, n))
}

impl InputTrack {
    pub fn death_scenario(&self, c: &mut ssx_assets::sfx::speech::game::DeathCauses) {
        c.wingsuit_deployed = self.wingsuit_deployed;
        c.wingsuit_gap = self.advised == Some(Advised::Wingsuit);
        c.ridge = self.advised == Some(Advised::Ridge);
        c.jump_short = self.advised == Some(Advised::JumpDeath);
        c.heli_grab = self.advised == Some(Advised::HeliGrab);
    }
}

pub const REVERB_ZONES: [&str; 3] = ["sfxreverbsml", "sfxreverbbig", "sfxtriggerout"];

pub fn reverb_zones(session: &Session, world: &World) -> [bool; 3] {
    REVERB_ZONES.map(|n| session.tracker.touching_name(&world.triggers, n))
}

#[derive(Clone, Debug, Default)]
pub struct FrameEvents {
    pub rider_dead: bool,
    pub ended_by_death: bool,
    pub reverb_zones: [bool; 3],
    pub advise_zones: [bool; ADVISE_ZONES.len()],
    pub sim_dt: f32,
    pub ground_material: i32,
}

pub fn ground_material(
    s: &RiderState,
    world: &World,
    objects: &ssx_assets::sfx_game::audio_objects::AudioObjects,
) -> i32 {
    let Some(c) = s.contact.as_ref() else { return 0 };
    let Some(a) = world.collision.entity(c.entity).and_then(|e| e.audio.as_ref()) else { return 0 };
    objects.get(&a.type_name, &a.sub_type).map_or(0, |m| m.board)
}

pub const GEOTAG_COLLECTED: (u32, &str) = (14683, "geotag_collected");

pub fn geotag_pickups(session: &Session, seen: &mut usize) -> usize {
    let n = session.geotags.as_ref().map_or(0, |g| g.result_list.len());
    let new = n.saturating_sub(*seen);
    *seen = n;
    new
}

pub fn end_of_game(session: &Session) -> bool {
    session.event.state == ssx_physics::event::EventState::Results
}

pub fn ended_by_death(session: &Session) -> bool {
    session.event_ended && session.event.result.as_ref().is_some_and(|r| r.failed)
}

fn avalanche_msg(s: &RiderState) -> AvalancheMsg {
    let p = &s.avalanche.published;
    let a = &p.aggregate;
    let o = a.orientation;
    let distance = p.aggregate_pos.distance(s.traj);
    AvalancheMsg {
        position: p.aggregate_pos.to_array(),
        orientation: [o.x, o.y, o.z, o.w],
        span: [a.span.x, a.span.z],
        speed: a.speed,
        mass: a.mass,
        probe_mass: p.probe.mass,
        distance,
    }
}

pub fn board_emitter(s: &RiderState) -> Emitter {
    let q = if s.graph.driven {
        s.graph.traj_rotation
    } else {
        s.traj_orientation * Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)
    };
    Emitter {
        bus: "Board_Center_BUS",
        settings: BOARD_CENTER,
        base: s.traj.to_array(),
        rotation: [q.x, q.y, q.z, q.w],
        velocity: [0.0; 3],
    }
}

pub fn rider_voice_emitter(s: &RiderState) -> Emitter {
    Emitter { bus: "Rider_Speech_BUS", settings: ssx_assets::sfx_dsp::sound3d::RIDER_VOICE, ..board_emitter(s) }
}

pub fn ride_audio(s: &RiderState, max_boost: f32, rewinding: bool, ev: FrameEvents, t: &mut InputTrack) -> RideAudio {
    let w = &s.weights;
    let weights = SurfaceWeights {
        windswept: w[0],
        crusty: w[1],
        wet: w[2],
        groomed: w[3],
        crud: w[4],
        powder: w[5],
        rock: w[6],
        ice: w[7],
        wood: w[8],
        metal: w[9],
        concrete: w[10],
        metal_hollow: w[11],
        coverage: w[13],
        heli: 0.0,
    };
    let awesome = s.score.is_awesome;
    if awesome && (!t.was_awesome || s.score.awesome_timer > t.tricky_len) {
        t.tricky_len = s.score.awesome_timer.max(1e-6);
    }
    t.was_awesome = awesome;
    use ssx_physics::rider::RideState as Rs;
    let in_ride = s.start.is_none() && !rewinding;
    let airborne = matches!(s.state, Rs::Airborne | Rs::Jump) && in_ride;
    let ground_before = t.ride_state.is_none_or(|p| p == Rs::Riding);
    let jumped = airborne && ground_before;
    t.ride_state = in_ride.then_some(s.state);
    let dt_ms = fctiwz(ev.sim_dt * 1000.0);
    let dead_latch = ev.rider_dead || t.dead_by_death_plane;
    for (k, inside) in ev.reverb_zones.into_iter().enumerate() {
        if inside && !t.zones_inside[k] {
            t.tunnel = [(true, false), (false, true), (false, false)][k];
        }
        t.zones_inside[k] = inside;
    }
    for (k, inside) in ev.advise_zones.into_iter().enumerate() {
        if inside && !t.advise_inside[k] {
            t.advised = Some(ADVISE_ZONES[k].1);
        }
        t.advise_inside[k] = inside;
    }
    if s.wingsuit.signal && t.advised == Some(Advised::Wingsuit) {
        t.wingsuit_deployed = true;
    }
    t.dead_by_death_plane = s.death.dead_by_death_plane;
    let sv = &s.survival;
    RideAudio {
        grounded: s.close.is_very_close && s.contact.is_some(),
        wingsuit: s.wingsuit.signal,
        dead_by_death_plane: dead_latch,
        awesome,
        super_awesome: s.score.is_super_awesome,
        boosting: s.boost.is_boosting,
        goofy: s.goofy,
        crashing: s.landing.is_bailing(),
        sliding: s.landing.backslide || s.graph.uber_bail_sliding,
        grinding: s.grind.is_grinding(),
        ice_axe: s.ice_axe.frame.active,
        ice_axe_blend: s.ice_axe.blend_weight,
        big_landing: s.score.is_big_landing,
        speed: s.player_speed,
        board_velocity_angle: s.board_velocity_angle,
        spray: s.spray.carving.length(),
        lean: s.kin.lean_norm,
        nose_tail: s.nose_tail,
        weights,
        boost_percent: if max_boost > 0.0 { s.score.boost / max_boost } else { 0.0 },
        awesome_percent: if awesome { s.score.awesome_timer / t.tricky_len } else { 0.0 },
        stick: ssx_physics::graph::stick_axes(s.stick_prev).to_array(),
        jumped,
        tricking: s.tricks.is_tricking,
        spin_speed: s.air.outputs.spin_speed,
        flip_speed: s.air.outputs.flip_speed,
        rewinding,
        time_dilation: 1.0,
        tunnel_small: t.tunnel.0,
        tunnel_large: t.tunnel.1,
        ground_material: ev.ground_material,
        ms_to_land: t.land_counter(s.ballistic.ticks, dt_ms, s.wingsuit.signal, !s.on_ground),
        ms_in_air: t.air_counter(!s.on_ground, dt_ms),
        vignettes: Vignettes {
            oxygen: sv.vignette_oxygen,
            temperature: sv.vignette_temperature,
            health: sv.vignette_health,
        },
        event_ended_by_death: ev.ended_by_death,
        avalanche: avalanche_msg(s),
        board_particle: s.board_particle.enabled,
        trick_update: s.score.trick_update.map(|r| (r.kind as i32, r.interaction)),
        ..RideAudio::default()
    }
}

pub fn music_state(s: &RiderState, session: &Session, rewind_backwards: bool) -> GameMusicState {
    let sv = &s.survival;
    GameMusicState {
        nis_active: s.start.is_some(),
        descent: s.score.descent,
        speed: s.player_speed,
        surface_rock: s.weights[ROCK],
        awesome: s.score.is_awesome,
        super_awesome: s.score.is_super_awesome,
        rewind_backwards,
        rewind_count: session.manager.count.rewinds_left,
        wingsuit: s.wingsuit.signal,
        in_air: !s.on_ground,
        grinding: s.grind.is_grinding(),
        ticks_to_landing: s.ballistic.valid.then_some(s.ballistic.ticks),
        track_finished: Some(session.flow.track_finished),
        armour: Some(sv.armour),
        oxygen_tank: Some(sv.oxygen_tank),
        vignette_oxygen: Some(sv.vignette_oxygen),
        temperature: Some(sv.temperature),
        temperature_max: Some(sv.temperature_max),
        race_started: false,
        // TODO(guess): without the results screens no result 1 (success), "failure" (2) once the death timer ended the event (0x830CCBD8 treats 2 and 3 alike); settle: running the front end's results flow around the ride (UI 0x1ED).
        result: if ended_by_death(session) { 2 } else { 0 },
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SpeechStep {
    pub race_start: bool,
    pub rider_dead: Option<ssx_physics::death::RiderDead>,
    pub death_timer_end: bool,
    pub flushed: [Option<ssx_physics::scoring::BanksFlushed>; 5],
}

impl SpeechStep {
    pub fn merge(a: Option<SpeechStep>, b: Option<SpeechStep>) -> Option<SpeechStep> {
        match (a, b) {
            (Some(a), Some(b)) => {
                let mut m = SpeechStep {
                    race_start: a.race_start || b.race_start,
                    rider_dead: b.rider_dead.or(a.rider_dead),
                    death_timer_end: a.death_timer_end || b.death_timer_end,
                    flushed: a.flushed,
                };
                for f in b.flushed.into_iter().flatten() {
                    if let Some(slot) = m.flushed.iter_mut().find(|s| s.is_none()) {
                        *slot = Some(f);
                    }
                }
                Some(m)
            }
            (a, b) => a.or(b),
        }
    }

    pub fn new(e: &ssx_physics::session::Execution, s: &RiderState) -> Self {
        SpeechStep {
            race_start: e.race_start,
            rider_dead: e.rider_dead,
            death_timer_end: e.timer == ssx_physics::death::DeathTimerEvent::EventEnds,
            flushed: s.score.flushed,
        }
    }
}

#[derive(Default)]
pub struct SpeechTrack {
    set_up: bool,
    crashing: bool,
    finished: bool,
    causes: ssx_physics::death::RiderDead,
    death_sent: bool,
    last_death: Option<ssx_assets::sfx::speech::game::DeathCauses>,
}

impl SpeechTrack {
    pub fn respawn(&mut self, sfx: &mut ssx_assets::sfx::SfxEngine) {
        if let Some(sp) = sfx.speech.as_mut() {
            sp.game.previous_death = self.last_death;
        }
        *self = Self { last_death: self.last_death, ..Self::default() };
    }
}

pub fn event_kind(session: &Session) -> i32 {
    session.event.setup.event_type.map_or(6, |t| ssx_assets::sfx_game::event_kind(t as u32))
}

pub fn speech_kind(session: &Session) -> ssx_assets::sfx::speech::game::EventKind {
    ssx_assets::sfx::speech::game::EventKind::from_kind(event_kind(session))
}

pub fn trick_msg(f: &ssx_physics::scoring::BanksFlushed) -> ssx_assets::sfx::speech::game::TrickMsg {
    ssx_assets::sfx::speech::game::TrickMsg {
        points_captured: f.is_points_captured,
        bad_landing: f.is_bad_landing,
        air_bank: f.air_bank,
        grind_bank: f.grind_bank,
        interactions_bank: f.interactions_bank,
    }
}

#[derive(Clone, Copy)]
pub struct SpeechInputs<'a> {
    pub s: &'a RiderState,
    pub world: &'a World,
    pub session: &'a Session,
    pub step: &'a SpeechStep,
    pub input: &'a InputTrack,
    pub r: &'a RideAudio,
    pub t: f32,
}

pub fn drive_speech(sfx: &mut ssx_assets::sfx::SfxEngine, f: SpeechInputs, track: &mut SpeechTrack) {
    use ssx_assets::sfx::speech::{game::DeathCauses, game::SpeechFrame, vars::id};
    let SpeechInputs { s, world, session, step, input, r, t } = f;
    let Some(sp) = sfx.speech.as_mut() else { return };
    if !track.set_up {
        track.set_up = true;
        let d = &world.survival;
        if d.cold {
            sp.set_game_var(id::IS_COLD_DROP, 1);
        }
        if d.low_oxygen {
            sp.set_game_var(id::IS_LOW_OXYGEN_DROP, 1);
        }
        if d.avalanche.avalanche_drop {
            sp.set_game_var(id::IS_AVALANCHE_DROP, 1);
        }
        let a = ssx_physics::avalanche::AvalancheSwitches::default();
        let probability = if matches!(a.tuning_set, 1 | 2) { a.carving_sensitivity } else { 0.0 };
        sp.event_setup(t, probability);
    }
    let bots = session.bots.as_ref().map_or(0, |b| b.bots.len() as i32);
    if step.race_start {
        sp.race_start(t, bots);
    }
    if let Some(m) = step.rider_dead {
        track.causes = m;
    }
    if step.death_timer_end && !track.death_sent {
        track.death_sent = true;
        let m = track.causes;
        let mut c = DeathCauses {
            health: m.by_health,
            freezing: m.by_freezing,
            asphyxia: m.by_asphyxia,
            death_volume: m.by_death_volume,
            ..DeathCauses::default()
        };
        input.death_scenario(&mut c);
        sp.death(&c);
        track.last_death = Some(c);
    }
    if r.crashing != track.crashing {
        track.crashing = r.crashing;
        sp.crash(r.crashing);
    }
    let airborne = !s.on_ground;
    for m in step.flushed.iter().flatten() {
        sp.trick_message(t, &trick_msg(m));
    }
    // TODO(guess): the results request completes in the frame it is sent; settle: the results flow's job queue's runner (`FUN_8241BAE8`) and the job's latency (`FUN_8251DA90` → `FUN_824B23C0`).
    let finished = session.event_ended;
    if finished && !track.finished {
        let scores = session.bots.as_ref().map_or(&[][..], |b| &b.scores[..]);
        let end = session.event.result.as_ref().and_then(|r| r.run_end(&session.event.setup, scores));
        bevy::log::info!("speech: RUN_END {end:?}");
        if let Some(end) = end {
            sp.run_end(&end, bots);
        }
    }
    track.finished = finished;
    let sv = &s.survival;
    let course = session.event.setup.course;
    let (distance, length) = course.map_or((0.0, 0.0), |c| (s.score.descent, c.start_y - c.end_y));
    let m = &r.avalanche;
    let f = SpeechFrame {
        time: t,
        speed: s.player_speed,
        distance,
        track_length: length,
        boost: s.score.boost,
        boost_enabled: r.boosting,
        awesome: r.awesome,
        dead: r.dead_by_death_plane || !s.death.alive,
        tunnel: r.tunnel_small || r.tunnel_large,
        rewinding: r.rewinding,
        vignette_temperature: sv.vignette_temperature,
        health: sv.health,
        vignette_oxygen: sv.vignette_oxygen,
        internal_temperature: sv.temperature,
        in_shade: false,
        airborne,
        avalanche: (m.mass > 0.0).then_some((m.distance, m.mass, m.speed, m.probe_mass)),
        race_ms: session.event.clock.running.then_some((session.event.clock.seconds * 1000.0) as i32),
        points: s.score.score,
        flight_distance: s.score.flight_distance,
        bots,
        paused: r.paused,
    };
    sfx.speech_frame(&f);
}

pub fn speech_enabled() -> bool {
    let mode = std::env::var("SSX_SPEECH").unwrap_or_default().to_ascii_lowercase();
    !matches!(mode.as_str(), "off" | "0" | "false")
}

pub fn drop_audio_helicopter(big_root: &std::path::Path, def: Option<&ssx_physics::event::EventDef>) -> u32 {
    use ssx_assets::sfx::speech::{DEFAULT_HELICOPTER, audio_helicopter};
    def.and_then(|d| audio_helicopter(big_root, &d.name).ok()).unwrap_or(DEFAULT_HELICOPTER)
}

pub fn speech_pilot(big_root: &std::path::Path, heli: u32) -> anyhow::Result<(i32, String)> {
    ssx_assets::sfx::speech::helicopter_pilot(big_root, heli)
}

pub fn speech_code(lang: ssx_ui::game::loc::Language) -> &'static str {
    use ssx_ui::game::loc::Language;
    match lang {
        Language::French => "fre",
        Language::German => "ger",
        Language::English | Language::Spanish | Language::Italian | Language::Japanese => "eng",
    }
}

#[derive(bevy::prelude::Resource, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpeechLanguage(&'static str);

impl SpeechLanguage {
    pub fn of(lang: Option<ssx_ui::game::loc::Language>) -> Self {
        let asked = std::env::var("SSX_SPEECH_LANG").ok();
        let asked = asked.and_then(|l| ["eng", "fre", "ger"].into_iter().find(|&code| code == l));
        SpeechLanguage(asked.unwrap_or_else(|| lang.map_or("eng", speech_code)))
    }

    pub fn code(self) -> &'static str {
        self.0
    }
}

#[cfg(test)]
mod speech_language_tests {
    use super::*;
    use ssx_ui::game::loc::Language;

    #[test]
    fn the_text_language_picks_the_speech_the_disc_has() {
        assert_eq!(speech_code(Language::French), "fre");
        assert_eq!(speech_code(Language::German), "ger");
        for l in [Language::English, Language::Spanish, Language::Italian, Language::Japanese] {
            assert_eq!(speech_code(l), "eng", "{l:?}");
        }
    }
}

pub struct SpeechDrop<'a> {
    pub big_root: &'a std::path::Path,
    pub character: &'a str,
    pub rider: &'a ssx_assets::sfx::RiderAudioInfo,
    pub track: &'a ssx_assets::sfx::TrackAudioInfo,
    pub pilot: (i32, String),
    pub session: &'a Session,
    pub drop: Option<u32>,
    pub deadly_descent: bool,
    pub rng: ssx_assets::sfx::speech::context::SpeechRandom,
    pub random: ssx_assets::career::random::Source,
    pub lang: SpeechLanguage,
}

impl SpeechDrop<'_> {
    pub fn setup(self) -> ssx_assets::sfx::speech::SpeechSetup {
        let voice = ssx_assets::content::GameContent::of(self.big_root).voice_of(self.character);
        let bank = match voice.as_str() {
            "travisrice" => "travis",
            "ty" => "tyler",
            n => n,
        };
        let big_root = self.big_root;
        ssx_assets::sfx::speech::SpeechSetup {
            lang: self.lang.code().to_string(),
            rider: bank.to_string(),
            rider_id: self.rider.speech_id,
            pilot_id: self.pilot.0,
            pilot: self.pilot.1,
            region_id: self.track.region_id,
            kind: speech_kind(self.session),
            deadly_descent: self.deadly_descent,
            rng: self.rng,
            random: self.random,
            drop_info: self
                .drop
                .and_then(|k| ssx_assets::sfx::speech::drop_audio_game_info(big_root, k).ok().flatten()),
        }
    }
}

pub fn nis_audio_type(name: &str) -> i32 {
    match name {
        "nis_avalanche_intro" => 2,
        "nis_heli_intro" | "nis_intro_long" => 1,
        "nis_snow_start_intro" => 3,
        "nis_deadly_descent_intro" => 5,
        _ => 0,
    }
}

pub fn event_nis_audio_type(big_root: &std::path::Path, scripts: &[String]) -> i32 {
    for s in scripts.iter().filter(|s| s.contains("eventtype_")) {
        let Ok(text) = std::fs::read_to_string(big_root.join(ssx_physics::nis::SCRIPTS_DIR).join(s)) else { continue };
        if let Some(at) = text.find("DO_SetNisAudioType(") {
            let rest = &text[at..];
            if let Some(name) = rest.split('"').nth(1) {
                return nis_audio_type(name);
            }
        }
    }
    0
}

#[derive(Clone, Debug, Default)]
pub struct IntroFrame {
    pub nis_message: Option<u8>,
    pub heli: Option<ssx_assets::sfx::heli::HeliTransform>,
    pub tags: Vec<String>,
}

struct NisScenes {
    assets: std::sync::Arc<ssx_physics::nis::IntroAssets>,
    tags: [Vec<ssx_assets::sfx::nis::NisAudioTag>; 2],
    inside: [bool; 2],
    ready: [bool; 2],
}

impl NisScenes {
    fn load(big_root: &std::path::Path, i: &std::sync::Arc<ssx_physics::nis::IntroAssets>) -> anyhow::Result<Self> {
        let mut out = Self { assets: i.clone(), tags: Default::default(), inside: [false; 2], ready: [false; 2] };
        for (k, scene) in [&i.nis.ready, &i.nis.jump].into_iter().enumerate() {
            let all = ssx_assets::sfx::nis::nis_audio_tags(big_root, &i.nis.bundles, &scene.sequence)?;
            out.tags[k] = all
                .into_iter()
                .filter(|t| {
                    t.role == ssx_assets::nis::ROLE_CHARACTER1 && t.system == ssx_assets::sfx::nis::ANT_TAGS_SYSTEM
                })
                .collect();
            let signals = ssx_assets::sfx::nis::nis_node_signals(big_root, &i.nis.bundles, &scene.node)?;
            out.inside[k] = i.heli.is_ok() && signals.iter().any(|n| n == "BLN_InsideHeli");
            out.ready[k] = signals.iter().any(|n| n == "BLN_IsNisReadyLoop");
        }
        Ok(out)
    }
}

#[derive(Default)]
pub struct IntroAudio {
    pub audio_type: i32,
    big_root: std::path::PathBuf,
    scenes: Option<NisScenes>,
    tracker: Option<(usize, ssx_physics::tags::TagTracker)>,
    flow: u8,
    reported: (u8, bool),
    heli: Option<ssx_assets::sfx::heli::HeliTransform>,
}

impl IntroAudio {
    pub fn load(
        big_root: &std::path::Path,
        intro: Option<&std::sync::Arc<ssx_physics::nis::IntroAssets>>,
        scripts: &[String],
    ) -> anyhow::Result<Self> {
        Ok(Self {
            audio_type: event_nis_audio_type(big_root, scripts),
            big_root: big_root.to_path_buf(),
            scenes: intro.map(|i| NisScenes::load(big_root, i)).transpose()?,
            ..Self::default()
        })
    }

    pub fn heli(&self) -> Option<ssx_assets::sfx::heli::HeliTransform> {
        self.heli
    }

    pub fn reported(&self) -> (u8, bool) {
        self.reported
    }

    pub fn tags(&self) -> impl Iterator<Item = &ssx_assets::sfx::nis::NisAudioTag> {
        self.scenes.iter().flat_map(|s| s.tags.iter().flatten())
    }

    pub fn frame(&mut self, s: &RiderState, heli: Option<&ssx_physics::helicopter::Helicopter>) -> IntroFrame {
        use ssx_assets::sfx::heli::HeliTransform;
        use ssx_physics::nis::Scene;
        let intro = s.start.as_ref().and_then(|l| l.intro.as_ref());
        if let Some(i) = intro
            && self.scenes.as_ref().is_none_or(|n| !std::sync::Arc::ptr_eq(&n.assets, &i.assets))
        {
            self.scenes = NisScenes::load(&self.big_root, &i.assets).ok();
            self.tracker = None;
        }
        let scenes =
            self.scenes.as_ref().filter(|n| intro.is_some_and(|i| std::sync::Arc::ptr_eq(&n.assets, &i.assets)));
        let k = intro.map(|i| usize::from(matches!(i.scene, Scene::Jump { .. })));
        let active = s.start.is_some();
        let ready = k.zip(scenes).is_some_and(|(k, n)| n.ready[k]);
        let now = match (active, ready) {
            (false, _) => 0u8,
            (true, false) => 1,
            (true, true) => 2,
        };
        let message = match (now, self.flow) {
            (0, 1 | 2) => Some(3),
            (1 | 2, 0) => Some(1),
            (2, 1) => Some(2),
            _ => None,
        };
        let mut f = IntroFrame { nis_message: message, ..IntroFrame::default() };
        if self.flow == 0 && self.reported.0 != 0 {
            self.audio_type = 0;
        }
        self.reported = (self.flow, k.zip(scenes).is_some_and(|(k, n)| n.inside[k]));
        self.flow = now;
        if let Some(h) = heli {
            let (q, p) = h.traj;
            self.heli = Some(HeliTransform::new(p.to_array(), [q.x, q.y, q.z, q.w], h.velocity.to_array()));
        }
        if let (Some(i), Some(k), Some(n)) = (intro, k, scenes) {
            if self.tracker.as_ref().is_none_or(|(sk, _)| *sk != k) {
                let windows: Vec<_> =
                    n.tags[k].iter().map(|t| ssx_physics::tags::TagWindow::new(t.time, t.duration, false)).collect();
                self.tracker = Some((k, ssx_physics::tags::TagTracker::new(&windows)));
            }
            if let Some((k, tr)) = self.tracker.as_mut()
                && tr.update(i.sequence_time(i.ticks()))
            {
                for (t, e) in tr.process() {
                    if e == ssx_physics::tags::TagEvent::Enter {
                        f.tags.push(n.tags[*k][t].name.clone());
                    }
                }
            }
        } else {
            self.tracker = None;
        }
        f.heli = self.heli;
        f
    }
}

pub fn nis_camera(s: &RiderState) -> Option<([f32; 3], [f32; 3])> {
    let intro = s.start.as_ref()?.intro.as_ref()?;
    let q = (s.traj_orientation * Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2)).normalize();
    let v = intro.camera((q, s.traj))?;
    Some((v.position.to_array(), (v.rotation * ssx_physics::Vec3::X).to_array()))
}

#[cfg(test)]
mod tests {
    use super::SpeechStep;

    #[test]
    fn a_carried_step_merges_into_the_next() {
        let carried = SpeechStep { race_start: true, death_timer_end: true, ..Default::default() };
        let next = SpeechStep { ..Default::default() };
        let m = SpeechStep::merge(Some(carried), Some(next)).expect("a step");
        assert!(m.race_start && m.death_timer_end);
        assert!(SpeechStep::merge(None, None).is_none());
        assert!(SpeechStep::merge(Some(carried), None).is_some());
    }
}
