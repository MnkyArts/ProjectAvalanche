use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::prelude::*;
use ssx_physics::event::EventState;
use ssx_physics::landing::LandingQuality;
use ssx_physics::rider::grind::tricks::GrindNode;
use ssx_physics::rider::{RideState, TrickInput};
use ssx_physics::surface::{SURFACE_NAMES, dominant_surface};
use ssx_physics::tricks::{TrickLevel, TrickPhase, TrickType};
use ssx_physics::{KMH_PER_M_PER_TICK, PORT_SUMMARY};

use super::{Clock, Ride};
use crate::camera::CameraMode;
use crate::rider::material::{CHARACTER_STATUS, RiderShading};
use crate::rider::skin::RiderRig;

fn event_hud(ride: &Ride) -> String {
    let (ev, s) = (&ride.session.event, &ride.state);
    let tutorial = match &ride.session.tutorial {
        Some(t) => {
            let mut out = String::new();
            if t.message.showing() {
                out += &format!("TUTORIAL {} / {}\n", t.message.title, t.message.body);
            }
            if !t.success.is_empty() {
                let items: Vec<String> =
                    t.success.iter().map(|r| format!("{} ({}/{})", r.text, r.count.min(r.needed), r.needed)).collect();
                out += &format!("TRICK {}: {}\n", t.help, items.join(", "));
            }
            out
        }
        None => String::new(),
    };
    let kind = ev.setup.event_type.map_or("no event", |t| t.name().trim_start_matches("EventType_"));
    let clock = |ms: u32| format!("{}:{:06.3}", ms / 60_000, (ms % 60_000) as f32 / 1000.0);
    let rivals = if ev.setup.rivals.is_empty() {
        String::new()
    } else {
        let list: Vec<String> = ev
            .setup
            .rivals
            .iter()
            .map(|&v| match ev.setup.event_type {
                Some(t) if t.is_race() => clock(v),
                _ => v.to_string(),
            })
            .collect();
        format!(" | rivals {}", list.join(" / "))
    };
    let splits: Vec<String> = ev.split_times.iter().map(|&(k, ms)| format!("#{} {}", k + 1, clock(ms))).collect();
    let mut out = tutorial
        + &match ev.state {
            EventState::Idle if ev.result.is_none() => format!(
                "EVENT {kind}: waiting for the start{rivals}
"
            ),
            _ => format!(
                "EVENT {kind}: {} | split {}/9, progress {:.0}%{rivals}{}
",
                clock(ev.clock.ms()),
                ev.split + 1,
                100.0 * ev.progress,
                match ride.session.survive {
                    Some(_) => format!(
                        " | loop {} ({:?}), descent {:.0} m",
                        ev.loops + 1,
                        ev.state,
                        ev.descent_mm(s.score.descent) as f32 / 1000.0
                    ),
                    None => String::new(),
                }
            ),
        };
    if !splits.is_empty() {
        out += &format!(
            "splits {}
",
            splits.join("  ")
        );
    }
    if ev.finish_message.is_some() {
        let survived = ev.setup.event_type == Some(ssx_physics::event::EventType::SurviveToDeath);
        out += if survived {
            ">>> SURVIVED <<<
"
        } else {
            ">>> FINISHED <<<
"
        };
    }
    if ev.track_finished && ev.result.is_none() {
        out += &format!(
            "finished {} | forced stop (stick pulled back, magnitude {:.2}) | AllowRaceEnd {}
",
            clock(ev.finish_ms.unwrap_or(0)),
            s.press.force_stop_mag,
            s.allow_race_end
        );
    }
    if let Some(r) = &ev.result {
        let place = match (r.rank, r.medal) {
            (Some(k), Some(m)) => {
                format!("place {} of {} - {} MEDAL", k + 1, r.rivals.len() + 1, format!("{m:?}").to_uppercase())
            }
            (Some(k), None) => format!("place {} of {} - no medal", k + 1, r.rivals.len() + 1),
            _ => "not ranked".into(),
        };
        out += &format!(
            "RESULT{}: time {} | points {} | descent {:.0} m | rewinds {} | {place}
",
            if r.failed { " (FAILED)" } else { "" },
            clock(r.time_ms),
            r.points,
            r.descent_mm as f32 / 1000.0,
            r.rewinds
        );
    }
    out
}

fn press_hud(ride: &Ride) -> String {
    let (s, p) = (&ride.state, &ride.state.press);
    let which = if p.inputs.nose {
        "nose"
    } else if p.inputs.tail {
        "tail"
    } else {
        "-"
    };
    format!(
        "board press: {}{} | blend {:.2} | IsBoardPress {} | press value {:+.2} | {:.1} s, {:.1} m | last {:.0}
",
        if p.pressing { "PRESSING " } else { "" },
        which,
        p.blend,
        p.is_board_press,
        s.grind.carving.press.value,
        s.score.press_time / 60.0,
        s.score.press_distance,
        ride.last_press_capture
    )
}

#[derive(Component)]
pub(super) struct RideHud;

#[derive(Component)]
pub(super) struct TrickHud;

#[derive(Resource)]
pub(super) struct RiderLabel(pub(super) String);

pub(super) fn spawn_hud(mut commands: Commands) {
    commands.spawn((
        TrickHud,
        Text::default(),
        TextFont { font_size: FontSize::Px(18.0), ..default() },
        TextColor(Color::WHITE),
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        Node {
            position_type: PositionType::Absolute,
            top: px(10),
            left: px(10),
            max_width: px(760),
            padding: UiRect::all(px(8)),
            ..default()
        },
    ));
    commands.spawn((
        RideHud,
        Text::default(),
        TextFont { font_size: FontSize::Px(15.0), ..default() },
        TextColor(Color::WHITE),
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        Node {
            position_type: PositionType::Absolute,
            bottom: px(10),
            right: px(10),
            max_width: px(560),
            padding: UiRect::all(px(8)),
            ..default()
        },
    ));
}

const RIDE_CONTROLS: &str = "A/D or Left/Right steer (air: spin) | S/Down brake, W/Up tuck (air: flip) | Space jump (on \
release; air: A grab) | grabs I/J/L/K = Y/X/B/A (ground / rail: J/L = X/B nose / tail press) | Shift R2 (boost / tweak) | Ctrl L2 (grind) | Ctrl+Shift signature (L2+R2) \
| E R1 (wingsuit) | hold Backspace rewind | Num8/4/6/2 right stick (grabs, up = jump) | pad: left stick, right stick grabs and up = jump, A/B/X/Y, R2, L2 grind, L2+R2, \
R1 wingsuit, L1 rewind | R respawn | N rail \
debug | Tab chase/fly | F wireframe | H overlay";

const RIDE_CONTROLS_CLASSIC: &str = "Classic: D-pad or A/D carve (air: spin) | S/Down brake, W/Up tuck (air: flip) | Space A jump \
(on release; air: A held wingsuit) | Q/E/Z/X = LB/RB/LT/RT grabs (exact shoulder sets; right stick (Num8/4/6/2) up/down = nose/tail) | J X \
(boost / tweak) | Ctrl Y grind | L B rewind | Ctrl+Shift signature | pad: D-pad, shoulders, right \
stick up/down nose/tail | R respawn | N rail debug | Tab chase/fly | F wireframe | H overlay";

fn death_hud(ride: &Ride) -> String {
    let (d, t) = (&ride.state.death, &ride.session.timer);
    if ride.session.event_ended {
        let why = if ride.session.event.result.as_ref().is_some_and(|r| !r.failed) {
            "the results are posted"
        } else {
            "the death timer ran out"
        };
        let outro = match ride.outro.as_ref() {
            Some(o) if o.assets.is_some() => format!(
                "outro NIS ({}; the results screen is not ported)",
                match o.reaction {
                    Some(0) => "win",
                    Some(2) => "lose",
                    _ => "neutral",
                }
            ),
            _ => "the game shows its results screen and outro".into(),
        };
        return format!("EVENT OVER: {why} ({outro}) | R respawns\n");
    }
    let hint = if t.hint { " | REWIND HINT: hold L1 / Backspace to rewind out of death" } else { "" };
    if !d.is_alive() {
        format!(
            "DEAD (death volume) {:.1} s{} | event ends in {:.1} s{hint}\n",
            d.time_since_death_by_death_plane / 60.0,
            if ride.state.carving.is_dead { ", carving IsDead" } else { "" },
            t.remaining.max(0.0),
        )
    } else if t.remaining >= 0.0 {
        format!("death timer {:.1} s (rewound {:.1} s){hint}\n", t.remaining, t.rewind_time)
    } else {
        String::new()
    }
}

fn grind_hud(ride: &Ride) -> String {
    let s = &ride.state;
    let (g, sc, t) = (&s.grind.carving, &s.score, &s.tricks.grind);
    let state = ["-", "approach", "GRINDING", "exit"].get(g.state as usize).copied().unwrap_or("?");
    let node = match t.node {
        GrindNode::None => "-".to_string(),
        n => format!("{n:?} {:.0} ticks, group {}", t.node_ticks, t.group),
    };
    format!(
        "grind: {state}{}{}{} | L2 {} | bank {:.0}, {:.1} m, {:.1} s | last capture {:.0} | grind trick {node}",
        if g.is_grinding { " (BLN_IsGrinding)" } else { "" },
        if g.super_grind { " super" } else { "" },
        if g.auto_grind { " auto" } else { "" },
        if s.grind.pad_l2 { "held" } else { "-" },
        sc.grind_bank,
        sc.grind_distance,
        sc.grind_time / 60.0,
        ride.last_grind_capture,
    )
}

fn trick_hud(ride: &Ride) -> String {
    let (s, max_boost) = (&ride.state, ride.world.tuning.scoring.max_boost);
    let (sc, t, l) = (&s.score, &s.tricks, &s.landing);
    let air = if s.state == RideState::Riding { "last air" } else { "air" };
    let ttl = s.air.prediction.map_or("-".to_string(), |p| format!("{:.0}", p.ticks));
    let trick = if t.phase == TrickPhase::None {
        "-".to_string()
    } else {
        let level = match t.level {
            TrickLevel::Normal => "",
            TrickLevel::Uber => " uber",
            TrickLevel::SuperUber => " super uber",
            TrickLevel::Signature => " signature",
        };
        let name = if t.trick_type != TrickType::None { t.trick_type } else { t.trick_type_locked };
        let name = match name {
            TrickType::Added(_) => format!("Added{} ({:?}'s places)", name.value(), name.flow_base()),
            _ => format!("{name:?}"),
        };
        format!("{name}{level} {:?}{} ({:.0} ticks)", t.phase, if t.tweak_node { " tweak" } else { "" }, t.phase_ticks)
    };
    let quality = match l.quality {
        LandingQuality::None => "-",
        LandingQuality::Sloppy => "sloppy",
        LandingQuality::Clean => "clean",
        LandingQuality::Tumble => "TUMBLE",
        LandingQuality::Bail => "BAIL",
        LandingQuality::UberCrash => "UBER CRASH",
    };
    let last = if sc.last_lost { format!("lost {:.0}", sc.last_capture) } else { format!("+{:.0}", sc.last_capture) };
    let tricky = match (sc.is_super_awesome, sc.is_awesome) {
        (true, _) => format!("SUPER TRICKY {:.1} s", sc.super_timer / 60.0),
        (false, true) => format!("TRICKY {:.1} s", sc.awesome_timer / 60.0),
        _ => format!("tricky meter {:.0}%", 100.0 * sc.boost / max_boost.max(1.0)),
    };
    format!(
        "{}{}{air}: spin {:.0} deg, flip {:.0} deg | flip/spin angle {:+.0}/{:+.0} deg | ticks to land {ttl} | {}\n\
         trick: {trick}\n\
         landing: {quality} (touchdown pitch {:+.0}, yaw {:+.0} deg) | balance pitch {:+.0}, spin {:+.0} deg\n\
         score {:.0} | trick bank {:.0} | last landing {last} | boost {:.1}/{max_boost:.0}{} | {tricky}\n{}{}\n{}",
        event_hud(ride),
        death_hud(ride),
        (sc.spin_fs + sc.spin_bs).to_degrees(),
        (sc.flip_fwd + sc.flip_bwd).to_degrees(),
        s.air.flip.angle.to_degrees(),
        s.air.spin.angle.to_degrees(),
        if s.goofy { "goofy" } else { "regular" },
        l.touchdown_pitch.to_degrees(),
        l.touchdown_spin.to_degrees(),
        l.pitch.to_degrees(),
        (l.spin - l.spin_target).to_degrees(),
        sc.score,
        sc.air_bank,
        sc.boost,
        if s.boost.is_boosting { " BOOSTING" } else { "" },
        press_hud(ride),
        grind_hud(ride),
        wingsuit_hud(ride),
    ) + &survival_hud(ride)
        + &avalanche_hud(ride)
}

fn survival_hud(ride: &Ride) -> String {
    let (s, d) = (&ride.state, &ride.world.survival);
    let (v, h) = (&s.survival, s.survival.hud());
    let mut line = format!("\nsurvival: health {:.2}/{:.2}", h.health, h.health_max);
    if h.armour > 0.0 || d.slots[ssx_physics::character::SLOT_ARMOUR] || d.rock_damage {
        line += &format!(" | armour {:.2}{}", h.armour, if h.armour < 0.001 { " DANGER" } else { "" });
    }
    if d.cold {
        let shade = if v.shade < 0.5 { "shade" } else { "sun" };
        line += &format!(" | temp {:.2}/{:.2} ({shade})", h.temperature, h.temperature_max);
    }
    if d.low_oxygen || v.oxygen < 1.0 {
        let r1 = if h.oxygen_use_requested { " R1" } else { "" };
        line += &format!(" | oxygen {:.2} (flow {:.0} m/s) tank {:.2}{r1}", v.oxygen, v.oxygen_flow, h.oxygen_tank);
    }
    if d.slots[ssx_physics::character::SLOT_ICE_AXE] {
        let a = &s.ice_axe;
        let state = if a.frame.active {
            "biting"
        } else if a.blend_weight >= 1.0 {
            "out"
        } else {
            "deploying"
        };
        line += &format!(" | ice axe {:.2} ({state})", h.ice_axe_effectiveness);
    }
    if d.slots[6] || d.slots[7] {
        line += &format!(" | lamp {:.2} goggles {:.2}", h.head_lamp_battery, h.pulse_goggles_battery);
    }
    line += &format!(
        " | vignettes {:.2}/{:.2}/{:.2}{}",
        v.vignette_health,
        v.vignette_temperature,
        v.vignette_oxygen,
        if s.death.is_alive() { "" } else { " | DEAD" }
    );
    line
}

fn avalanche_hud(ride: &Ride) -> String {
    let (s, sim) = (&ride.state, &ride.session.avalanche);
    let av = &s.avalanche;
    let (slabs, sloughs) =
        (sim.slabs.iter().filter(|p| p.alive()).count(), sim.sloughs.iter().filter(|p| p.alive()).count());
    if av.switches.tuning_set == ssx_physics::avalanche::DEFAULT_TUNING_SET && slabs + sloughs == 0 {
        return String::new();
    }
    let set = match av.switches.tuning_set {
        2 => "medium",
        3 => "small",
        _ => "large",
    };
    let p = &av.published;
    let tumbling = s.landing.bail_on_landing && av.bailed_because_of_avalanche;
    format!(
        "\navalanche ({set}, sensitivity {:.0}): mass {:.1} at {} | probe {:.2} | A {:.2}{} | damage {:.1} | \
         {slabs} slabs, {sloughs} sloughs{}",
        av.switches.carving_sensitivity,
        p.aggregate.mass,
        if slabs + sloughs > 0 { format!("{:.0} m", p.aggregate_pos.distance(s.traj)) } else { "-".into() },
        p.probe.mass,
        av.frame.value,
        if tumbling { " TUMBLING" } else { "" },
        av.damage_running_total,
        if ride.motion_blur_cue < 0 {
            format!(" | motion blur cued off ({})", ride.motion_blur_cue)
        } else {
            String::new()
        },
    )
}

fn wingsuit_hud(ride: &Ride) -> String {
    let (s, ws) = (&ride.state, &ride.state.wingsuit);
    let node = match s.state {
        RideState::WingsuitEnter => "DEPLOY",
        RideState::WingsuitFly => "FLY",
        RideState::WingsuitExit => "RETRACT",
        _ if ws.can_use() => "ready (R1 in the air)",
        _ => "not usable yet (race not properly started)",
    };
    let att = ws.last.filter(|_| ws.signal).map_or(String::new(), |o| {
        format!(" | roll {:+.2} pitch {:+.2} yaw {:+.2}", o.attitude.x, o.attitude.y, o.attitude.z)
    });
    format!(
        "wingsuit: {node} | R1 {} | time {:.1} s, use {:.0}% | effectiveness {:.3} | lift {:.1} m/s per step{att}",
        if ws.hold { "held" } else { "-" },
        ws.time / 60.0,
        100.0 * ws.use_percentage,
        ws.effectiveness,
        ws.lift_velocity.length() * 60.0,
    )
}

fn grab_label(t: &TrickInput) -> String {
    let mut parts = Vec::new();
    if t.right_stick.length() > 0.1 {
        parts.push(format!("RS {:+.2},{:+.2}", t.right_stick.x, t.right_stick.y));
    }
    for (held, name) in [(t.y, "Y"), (t.x, "X"), (t.b, "B"), (t.a, "A"), (t.tweak, "R2"), (t.signature, "L2+R2")] {
        if held {
            parts.push(name.to_string());
        }
    }
    if parts.is_empty() { String::new() } else { format!(" | grab {}", parts.join(" ")) }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn update_hud(
    ride: Res<Ride>,
    label: Res<RiderLabel>,
    rig: Option<Res<RiderRig>>,
    shading: Option<Res<RiderShading>>,
    mode: Res<CameraMode>,
    diagnostics: Res<DiagnosticsStore>,
    real: Res<Time<Real>>,
    mut last: Local<Option<f64>>,
    mut text: Single<&mut Text, With<RideHud>>,
) {
    if !hud_due(&ride, &real, &mut last) {
        return;
    }
    let s = &ride.state;
    let surface = match s.contact {
        Some(_) => {
            let (i, w) = dominant_surface(&s.weights);
            format!("{} {:.2}", SURFACE_NAMES[i], w)
        }
        None => "no contact".to_string(),
    };
    let frame_ms = diagnostics.get(&FrameTimeDiagnosticsPlugin::FRAME_TIME).and_then(|d| d.smoothed()).unwrap_or(0.0);
    let i = &ride.input;
    let source = if ride.fixed_input.is_some() { "fixed (--ride-input)" } else { "live" };
    let clock = match ride.clock {
        Clock::Fixed => "30 Hz fixed step, 2 ticks",
        Clock::PerFrame { sub: 1, .. } => "screenshot: 1 step per frame",
        Clock::PerFrame { .. } => "screenshot: SSX_RIDE_SUBFRAMES frames per step",
    };
    let camera = if *mode == CameraMode::Chase { "chase" } else { "fly" };
    let controls = if ride.classic() { RIDE_CONTROLS_CLASSIC } else { RIDE_CONTROLS };
    let new = format!(
        "RIDE  {:.1} km/h | {} | ground d {} | surface {surface} | lean {:+.1} deg\n\
         t {:.1} s | distance {:.0} m | height {:.0} m | camera {camera}\n\
         input ({source}): steer {:+.2} brake {:.2} tuck {:.2}{}{}\n\
         physics ({clock}): {:.3} ms/step | {frame_ms:.1} ms/frame\n\
         ride mode: physics port, simplifications: {PORT_SUMMARY}\n{}{}{}\n{controls}",
        s.speed() * KMH_PER_M_PER_TICK,
        match (s.state, s.stopped) {
            _ if s
                .start
                .as_ref()
                .and_then(|l| l.intro.as_ref())
                .is_some_and(|i| { matches!(i.scene, ssx_physics::nis::Scene::Ready { .. }) }) =>
            {
                "INTRO: in the helicopter's door (W / A / right stick up, Num8: GO)"
            }
            _ if s.start.as_ref().is_some_and(|l| l.intro.is_some()) => "INTRO: the jump",
            _ if s.start.as_ref().is_some_and(|l| l.idle()) => "LEVEL START (W / A / right stick up, Num8: GO)",
            _ if s.start.is_some() => "GO",
            _ if ride.rewinding => "REWIND",
            _ if ride.session.event_ended => "EVENT OVER",
            _ if ride.session.event.track_finished => "FINISHED (forced stop)",
            _ if !s.death.is_alive() => "DEAD",
            _ if s.janked.janked => "JANKED (hold Backspace to rewind)",
            _ if s.landing.is_bailing() => "BAIL",
            (RideState::Riding, false) => "riding",
            (RideState::Riding, true) => "stopped (W to get going)",
            (RideState::Airborne, _) => "AIR",
            (RideState::Jump, _) => "JUMP",
            (RideState::WingsuitEnter, _) => "WINGSUIT (deploy)",
            (RideState::WingsuitFly, _) => "WINGSUIT",
            (RideState::WingsuitExit, _) => "WINGSUIT (retract)",
        },
        if s.close.d < 1e3 { format!("{:.2} m", s.close.d) } else { "-".to_string() },
        s.lean.to_degrees(),
        ride.seconds(),
        ride.distance,
        s.position.y,
        i.steer,
        i.brake,
        i.tuck,
        if i.jump { " jump held" } else { "" },
        grab_label(&ride.tricks),
        ride.step_ms,
        label.0,
        rig.map_or(String::new(), |r| format!(
            "\ncharacter: {}\n{} | {:.3} ms/frame",
            r.summary,
            r.anim.label(),
            r.anim_ms
        )),
        shading.map_or(String::new(), |sh| format!(
            "\ndynamicAOMult {:.2} | dynamicGlowMult.y {:.0}\n{CHARACTER_STATUS}",
            sh.ao, sh.glow
        )),
    );
    if text.0 != new {
        text.0 = new;
    }
}

fn hud_due(ride: &Ride, real: &Time<Real>, last: &mut Option<f64>) -> bool {
    let now = real.elapsed_secs_f64();
    let due = matches!(ride.clock, Clock::PerFrame { .. }) || last.is_none_or(|t| now - t >= HUD_PERIOD_S);
    if due {
        *last = Some(now);
    }
    due
}

const HUD_PERIOD_S: f64 = 0.1;

pub(super) fn update_trick_hud(
    ride: Res<Ride>,
    gear: Res<GearHud>,
    ghosts: Option<Res<crate::rider::ghosts::GhostHud>>,
    real: Res<Time<Real>>,
    mut last: Local<Option<f64>>,
    mut text: Single<&mut Text, With<TrickHud>>,
) {
    if !hud_due(&ride, &real, &mut last) {
        return;
    }
    let new = trick_hud(&ride)
        + &gear.lamp
        + &gear.screen
        + ghosts.as_ref().map_or("", |g| g.line.as_str())
        + &crate::ride::bots::hud_line(&ride);
    if text.0 != new {
        text.0 = new;
    }
}

#[derive(Resource, Default)]
pub(super) struct GearHud {
    pub(super) lamp: String,
    pub(super) screen: String,
}
