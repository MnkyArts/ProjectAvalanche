use crate::avm1::Ctx;
use crate::avm1::value::Value;
use crate::game::fe::{FeEnv, FrontEnd, MovieRequest, screens};
use crate::game::values::UiValues;

pub const ATTRACT_SECONDS: f64 = 30.0;
pub const BOOT_MOVIE: &str = "boot_movie_eng";
pub const ATTRACT_MOVIE: &str = "intromovie";
pub const ESRB_BINDING: &str = "db_boot_esrb";
const LOGO_DATA: &str = "dp_boot_logo_screen_data";
pub const ESRB_READY: &str = "dp_boot_esrb_ready";

const BOOT_LOGO_STARTED: u16 = 0x2F;
const BOOT_FLOW_STARTED: u16 = 0x30;
const BOOT_SKIP_INTRO: u16 = 0x31;
const BACKGROUND_TRANSITION: u16 = 0x2E;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EsrbValues {
    pub played: bool,
    pub handle_input: bool,
    pub play_intro: bool,
    pub attract_started: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Esrb {
    pub played: bool,
    pub started: bool,
    pub bound: bool,
}

#[derive(Clone, Debug)]
pub struct BootFlow {
    pub attract_off: bool,
    pub attract: f64,
    pub play_intro: bool,
    pub handle_input: bool,
    pub signed_in: bool,
    pub esrb: Option<Esrb>,
    pub esrb_values: EsrbValues,
}

impl BootFlow {
    pub fn new() -> Self {
        Self::of(ssx_assets::data_roots::title_update().is_some())
    }

    pub fn of(title_update: bool) -> Self {
        BootFlow {
            attract_off: true,
            attract: ATTRACT_SECONDS,
            play_intro: false,
            handle_input: false,
            signed_in: false,
            esrb: title_update.then(Esrb::default),
            esrb_values: EsrbValues::default(),
        }
    }
}

impl Default for BootFlow {
    fn default() -> Self {
        Self::new()
    }
}

pub fn set_launch_data(values: &mut UiValues, skip_intro: bool) {
    values.set_bool(BOOT_SKIP_INTRO, skip_intro);
}

pub fn activate(fe: &mut FrontEnd, env: &mut FeEnv) {
    fe.screen = screens::id::BOOT;
    if env.values.bool(BOOT_LOGO_STARTED) {
        if let Some(e) = fe.boot.esrb.as_mut() {
            let v = fe.boot.esrb_values;
            e.played = v.played;
            fe.boot.handle_input = v.handle_input;
            fe.boot.play_intro = v.play_intro;
            fe.boot.attract_off = v.attract_started;
        }
        return;
    }
    env.values.set_bool(BOOT_LOGO_STARTED, true);
    crate::game::fe::settings::init_values(env.values, &fe.audio_store);
    if !env.values.bool(BOOT_SKIP_INTRO) {
        fe.queue_movie(MovieRequest { can_stop: false, ..MovieRequest::new(BOOT_MOVIE, "boot") });
        fe.note(format!("FLOW_PlayMovie {BOOT_MOVIE}"));
        fe.boot.play_intro = false;
        fe.boot.handle_input = false;
        fe.publish.push(LOGO_DATA.into());
    } else {
        fe.boot.play_intro = true;
        fe.boot.handle_input = true;
        fe.boot.attract_off = false;
        fe.publish.push(LOGO_DATA.into());
        if fe.boot.esrb.is_some() {
            fe.publish.push(ESRB_READY.into());
        }
    }
    env.nav_events.push("backgroundcontainer_enable".into());
}

pub fn update(fe: &mut FrontEnd, env: &mut FeEnv, dt: f64) {
    if let Some(e) = fe.boot.esrb.as_mut()
        && !e.played
    {
        if e.bound && e.started {
            e.started = false;
            esrb_done(fe, env);
        }
        return;
    }
    if fe.boot.attract_off || env.values.bool(BOOT_FLOW_STARTED) {
        return;
    }
    fe.boot.attract -= dt;
    if fe.boot.attract - dt <= 0.0 {
        fe.queue_movie(MovieRequest::new(ATTRACT_MOVIE, "attract"));
        fe.note(format!("FLOW_PlayMovie {ATTRACT_MOVIE} (attract)"));
        fe.boot.attract_off = true;
        fe.boot.handle_input = false;
        fe.boot.play_intro = false;
        fe.publish.push(LOGO_DATA.into());
    }
}

fn esrb_done(fe: &mut FrontEnd, env: &mut FeEnv) {
    fe.boot.esrb_values = EsrbValues { played: true, handle_input: true, play_intro: true, attract_started: false };
    fe.note("the ESRB notice is done: go_to_title_screen");
    env.nav_events.push("go_to_title_screen".into());
}

pub fn action(fe: &mut FrontEnd, env: &mut FeEnv, action: &str, param: f64) {
    match action {
        "A_BootStart" => {
            // PORT(choice): a start without a device (the boot logo's own button `mcNextButton` clicked with the
            // pointer: `OnStartPressed` gets no argument, the parameter is NaN, which the game's conversion makes
            // controller 0) signs in the PC's keyboard and pointer (device 4), so that no absent pad 0 is taken for the
            // player's; the 360 has no pointer: its mouse device is never connected.
            let device = if param.is_nan() { i32::from(crate::game::input::KEYBOARD_DEVICE) } else { param as i32 };
            start(fe, env, device);
            env.values.set_int(BACKGROUND_TRANSITION, 1);
            env.values.set_bool(BOOT_FLOW_STARTED, true);
        }
        "A_GameMovie" => {
            if fe.boot.esrb.is_some_and(|e| !e.played) {
                fe.publish.push(ESRB_READY.into());
            } else {
                fe.boot.play_intro = true;
                fe.boot.handle_input = true;
                fe.publish.push(LOGO_DATA.into());
            }
            fe.boot.attract_off = false;
            fe.boot.attract = ATTRACT_SECONDS;
        }
        "A_BootESRBStart" => {
            if let Some(e) = fe.boot.esrb.as_mut() {
                e.started = true;
                fe.note("the ESRB notice starts");
            }
        }
        "A_BootESRBDone" if fe.boot.esrb.is_some() => esrb_done(fe, env),
        _ => {}
    }
}

pub fn bind(fe: &mut FrontEnd, bound: bool) {
    if let Some(e) = fe.boot.esrb.as_mut() {
        e.bound = bound;
        fe.note(if bound { "the ESRB notice's view is bound" } else { "the ESRB notice's view is unbound" });
    }
}

fn start(fe: &mut FrontEnd, env: &mut FeEnv, device: i32) {
    if env.values.bool(BOOT_FLOW_STARTED) {
        return;
    }
    env.values.set_int(0x229, device);
    env.values.set_bool(0x22A, true);
    fe.note(format!("message 0xDD9DA034; controller {device}"));
    // PORT(choice): the local profile counts as signed in; no Live in the port: the Xbox Live / profile sign-in
    // (0x8287CBF8: `profile_signin`, a popup) is not ported.
    fe.boot.signed_in = true;
    fe.note("FE manager event 10 (message 0xBA30389A)");
    crate::game::fe::flow::signed_in(fe, env, device);
}

pub fn build(fe: &FrontEnd, cx: &mut Ctx) -> Value {
    let o = Value::Object(cx.new_object());
    let _ = cx.set_member(&o, "bShouldPlayScreenIntro", Value::Bool(fe.boot.play_intro));
    let _ = cx.set_member(&o, "bShouldScreenHandleInput", Value::Bool(fe.boot.handle_input));
    o
}

pub fn esrb_ready(fe: &FrontEnd) -> Option<Value> {
    fe.boot.esrb.map(|_| Value::Bool(true))
}
