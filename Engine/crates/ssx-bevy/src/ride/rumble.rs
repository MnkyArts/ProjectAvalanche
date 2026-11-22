use std::io::Write;
use std::time::Duration;

use bevy::ecs::system::SystemParam;
use bevy::input::gamepad::{Gamepad, GamepadRumbleIntensity, GamepadRumbleRequest};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use ssx_physics::rider::PadRumble;
use ssx_physics::session::LOOP_STEP_S;

use super::Ride;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct RumblePost {
    motors: Option<[f32; 2]>,
    executed: Option<(PadRumble, bool)>,
    stop_all: bool,
}

impl RumblePost {
    pub(super) fn executed(&mut self, rumble: &PadRumble, rewinding: bool) {
        let m = rumble.motors;
        self.executed = Some((*rumble, rewinding));
        self.motors = (m[0] > 0.0 || m[1] > 0.0).then_some(m);
    }

    pub(super) fn restart(&mut self) {
        *self = Self { stop_all: true, ..Self::default() };
    }
}

pub fn motor_word(v: f32) -> u16 {
    (v * 65_535.0) as i64 as u16
}

fn word_ratio(word: u16) -> f32 {
    if word == 0 { 0.0 } else { (f32::from(word) + 0.5) / 65_535.0 }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Send(Entity, [u16; 2]);

#[derive(Clone, Copy, Debug, PartialEq)]
struct PadMotors {
    pad: Entity,
    active: bool,
    timer: f32,
    words: [u16; 2],
}

#[derive(Resource, Default)]
pub struct PadRumbleOut {
    pads: Vec<PadMotors>,
    log: Option<std::io::BufWriter<std::fs::File>>,
    step: u64,
}

pub type RealPad = (With<Gamepad>, Without<crate::shell::game_mode::ScriptPad>);

#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub enum RumbleTarget {
    #[default]
    Focused,
    Pads(Vec<Entity>),
    Unattended,
}

// PORT(choice): a test run (`--autoplay`, `--script`, …) or a screenshot ride sends no rumble to a real pad unless it
// is named: an automated ride never shakes the pad on the desk. Its scripts' pad, the signed-in one, has no motors; the
// console has no unattended runs.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct RumbleTestPad(pub Option<String>);

// PORT(choice): every rumble a pad is sent lasts at most 200 ms in gilrs (the effect's own length) unless the next step
// sends it again: a frame loop that stalls (a hitch, a debugger) stops the motors itself. The console's loop never
// stalls; its motors run until the input manager's timer stops them (3 steps after the last post, 100 ms).
const WATCHDOG: Duration = Duration::from_millis(200);

impl PadRumbleOut {
    fn slot(&mut self, pad: Entity) -> &mut PadMotors {
        if let Some(i) = self.pads.iter().position(|p| p.pad == pad) {
            return &mut self.pads[i];
        }
        self.pads.push(PadMotors { pad, active: false, timer: 0.0, words: [0; 2] });
        let last = self.pads.len() - 1;
        &mut self.pads[last]
    }

    fn update(&mut self, dt: f32, sends: &mut Vec<Send>) {
        for p in self.pads.iter_mut().filter(|p| p.active) {
            if p.timer < 0.0 {
                (p.active, p.timer, p.words) = (false, 0.0, [0; 2]);
                sends.push(Send(p.pad, [0; 2]));
            } else {
                p.timer -= dt;
            }
        }
    }

    fn start(&mut self, pad: Entity, duration: f32, motors: [f32; 2], sends: &mut Vec<Send>) {
        let words = motors.map(motor_word);
        let p = self.slot(pad);
        (p.active, p.timer, p.words) = (true, duration, words);
        sends.push(Send(pad, words));
    }

    fn stop_all(&mut self, sends: &mut Vec<Send>) {
        for p in &mut self.pads {
            (p.active, p.timer, p.words) = (false, 0.0, [0; 2]);
            sends.push(Send(p.pad, [0; 2]));
        }
    }

    fn retarget(&mut self, connected: &[Entity], targets: &[Entity], sends: &mut Vec<Send>) {
        self.pads.retain(|p| connected.contains(&p.pad));
        for p in self.pads.iter_mut().filter(|p| !targets.contains(&p.pad) && p.words != [0; 2]) {
            (p.active, p.timer, p.words) = (false, 0.0, [0; 2]);
            sends.push(Send(p.pad, [0; 2]));
        }
    }

    fn log(&mut self, what: std::fmt::Arguments) {
        if let Some(f) = self.log.as_mut() {
            let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
            let _ = writeln!(f, "{:.6}\t{}\t{what}", t.as_secs_f64(), self.step);
            if self.step.is_multiple_of(30) {
                let _ = f.flush();
            }
        }
    }
}

// PORT(choice): the sends go through Bevy's rumble requests to gilrs, whose force-feedback thread sets each pad every
// 50 ms (its tick) to the sum of the pad's running effects (Linux: an evdev `FF_RUMBLE` effect of 100 ms; Windows:
// `Windows.Gaming.Input` `SetVibration`): a send reaches the motors 0…50 ms later and the 30 Hz values are sampled at
// 20 Hz. The console's input manager called `XamInputSetState` with each step's words.
fn requests(sends: &[Send], out: &mut MessageWriter<GamepadRumbleRequest>) {
    for &Send(gamepad, [left, right]) in sends {
        out.write(GamepadRumbleRequest::Stop { gamepad });
        if [left, right] != [0; 2] {
            let intensity = GamepadRumbleIntensity { strong_motor: word_ratio(left), weak_motor: word_ratio(right) };
            out.write(GamepadRumbleRequest::Add { gamepad, intensity, duration: WATCHDOG });
        }
    }
}

#[derive(SystemParam)]
pub(super) struct RumblePads<'w, 's> {
    gamepads: Query<'w, 's, (Entity, &'static Name), RealPad>,
    target: Res<'w, RumbleTarget>,
    test_pad: Res<'w, RumbleTestPad>,
    window: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
}

impl RumblePads<'_, '_> {
    fn get(&self, screenshot: bool) -> (Vec<Entity>, Vec<Entity>) {
        let connected: Vec<Entity> = self.gamepads.iter().map(|(e, _)| e).collect();
        let named = || match &self.test_pad.0 {
            Some(n) => self.gamepads.iter().filter(|(_, name)| name.as_str() == n).map(|(e, _)| e).collect(),
            None => Vec::new(),
        };
        let targets = match &*self.target {
            RumbleTarget::Pads(t) => t.iter().copied().filter(|e| connected.contains(e)).collect(),
            RumbleTarget::Unattended => named(),
            RumbleTarget::Focused if screenshot => named(),
            // PORT(choice): without the game shell (the viewer's ride) every connected gamepad rumbles while the
            // window has the focus; the console's simulation module sends to its signed-in user's pad only.
            RumbleTarget::Focused if self.window.iter().all(|w| w.focused) => connected.clone(),
            RumbleTarget::Focused => Vec::new(),
        };
        (connected, targets)
    }
}

fn loop_step(
    out: &mut PadRumbleOut,
    post: RumblePost,
    (connected, targets): (Vec<Entity>, Vec<Entity>),
    writer: &mut MessageWriter<GamepadRumbleRequest>,
) {
    let mut sends = Vec::new();
    if post.stop_all {
        out.stop_all(&mut sends);
    }
    out.update(LOOP_STEP_S, &mut sends);
    if let Some(m) = post.motors {
        for &pad in &targets {
            out.start(pad, LOOP_STEP_S, m, &mut sends);
        }
    }
    out.retarget(&connected, &targets, &mut sends);
    if post.executed.is_some() || !sends.is_empty() {
        let (r, rewinding) = post.executed.unwrap_or_default();
        let (src, i) = (r.sources, r.sources.carving_inputs);
        let a = i.avalanche_vel.length() * i.avalanche_mass;
        out.log(format_args!(
            "motors\t{:.6}\t{:.6}\tcarving\t{:.6}\t{:.6}\tsurface\t{}\tuber\t{}\tdead\t{}\tspeed\t{:.4}\tavalanche\t{a:.4}\t\
             grind\t{}\tlanding\t{:.6}\t{:.1}/{:.1}\tw\t{:.3}\tbail_exit\t{}\trewind\t{rewinding}\tsends\t{sends:?}",
            r.motors[0],
            r.motors[1],
            src.carving[0],
            src.carving[1],
            i.surface,
            i.uber_sliding,
            i.dead_on_ground,
            i.speed,
            i.grind_state,
            src.landing,
            r.landing_timer,
            r.landing_total_time,
            r.landing_weight,
            src.bail_exit,
        ));
    }
    requests(&sends, writer);
}

pub(super) fn rumble_step(
    mut out: ResMut<PadRumbleOut>,
    ride: Option<ResMut<Ride>>,
    pads: RumblePads,
    mut writer: MessageWriter<GamepadRumbleRequest>,
) {
    let post = match ride {
        Some(r) if matches!(r.clock, super::Clock::PerFrame { .. }) => return,
        Some(mut r) => {
            out.step = r.steps;
            std::mem::take(&mut r.rumble)
        }
        None => RumblePost::default(),
    };
    loop_step(&mut out, post, pads.get(false), &mut writer);
}

pub(super) fn rumble_shot_step(
    mut out: ResMut<PadRumbleOut>,
    mut ride: ResMut<Ride>,
    pads: RumblePads,
    mut writer: MessageWriter<GamepadRumbleRequest>,
) {
    if !matches!(ride.clock, super::Clock::PerFrame { .. })
        || (out.step == ride.steps && ride.rumble == RumblePost::default())
    {
        return;
    }
    out.step = ride.steps;
    let post = std::mem::take(&mut ride.rumble);
    loop_step(&mut out, post, pads.get(true), &mut writer);
}

// PORT(choice): the motors stop when the window loses the focus, when the game's exit is asked, at `AppExit` and when
// the ride ends without its level's shutdown; an unplugged pad is forgotten. On the console the guide button pauses
// the game (no post: the timer stops the motors within 3 steps) and the dashboard ends the title.
pub(super) fn rumble_frame(
    mut out: ResMut<PadRumbleOut>,
    ride: Option<Res<Ride>>,
    pads: RumblePads,
    mut exits: MessageReader<AppExit>,
    mut writer: MessageWriter<GamepadRumbleRequest>,
) {
    let mut sends = Vec::new();
    let screenshot = ride.as_ref().is_some_and(|r| matches!(r.clock, super::Clock::PerFrame { .. }));
    let (connected, targets) = pads.get(screenshot);
    let exiting = exits.read().count() > 0;
    if exiting || ride.is_none() {
        out.retarget(&connected, &[], &mut sends);
    } else {
        out.retarget(&connected, &targets, &mut sends);
    }
    if !sends.is_empty() {
        out.log(format_args!("stop\t{sends:?}"));
        if exiting && let Some(f) = out.log.as_mut() {
            let _ = f.flush();
        }
    }
    requests(&sends, &mut writer);
}

pub(super) fn register(app: &mut App) {
    let log = std::env::var_os("SSX_RUMBLE_LOG").and_then(|p| match std::fs::File::create(&p) {
        Ok(f) => Some(std::io::BufWriter::new(f)),
        Err(e) => {
            warn!("SSX_RUMBLE_LOG: {} not written: {e}", p.to_string_lossy());
            None
        }
    });
    let test_pad = std::env::var("SSX_RUMBLE_PAD").ok().filter(|n| !n.is_empty());
    app.insert_resource(PadRumbleOut { log, ..PadRumbleOut::default() })
        .insert_resource(RumbleTestPad(test_pad))
        .init_resource::<RumbleTarget>()
        .add_systems(FixedUpdate, rumble_step.after(super::RideStepSet))
        .add_systems(Update, rumble_shot_step.after(super::RideStepSet).run_if(resource_exists::<Ride>))
        .add_systems(PostUpdate, rumble_frame.before(bevy::gilrs::RumbleSystems));
}

#[cfg(test)]
mod tests;
