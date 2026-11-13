use super::*;

#[derive(Clone, Debug, PartialEq)]
pub struct RemixEvent {
    pub name: &'static str,
    pub value: f32,
}

#[derive(Default)]
pub struct RiderSounds {
    started: bool,
    prev: u32,
    board: Option<CsisHandle>,
    grind: Option<CsisHandle>,
    sweet: Option<(CsisHandle, i32)>,
    slide: Option<CsisHandle>,
    tumble: Option<CsisHandle>,
    awesome_global: Option<CsisHandle>,
    wingsuit: Option<CsisHandle>,
    ice_scrape: Option<CsisHandle>,
    big_air: Option<CsisHandle>,
    boost_latch: bool,
    awesome_boost_latch: bool,
    dilating: bool,
    trick_latch: bool,
    pub run_ended: bool,
    crash: bool,
    impact_sent: bool,
    axe_latch: bool,
    axe_prev_blend: f32,
    pub remix: Vec<RemixEvent>,
    pub mode: u8,
    pub snapshots: crate::sfx_bus::SnapshotDirector,
    eog: bool,
    flare_latch: bool,
}

pub(super) fn call(sfx: &mut SfxEngine, name: &str, params: &[i32]) -> Option<CsisHandle> {
    sfx.call(name, params)
}

pub(super) fn stop(sfx: &mut SfxEngine, h: &mut Option<CsisHandle>) {
    if let Some(h) = h.take() {
        sfx.stop(h);
    }
}

impl RiderSounds {
    fn start(&mut self, sfx: &mut SfxEngine, spd: i32, cls4: i32) {
        self.slide = call(sfx, "SFX_Slide", &[spd, cls4]);
        self.tumble = call(sfx, "SFX_Tumble", &[spd, cls4, 1]);
        self.awesome_global = call(sfx, "SFX_Awesome_Global", &[]);
        sfx.fire_event("trigger_Trick_SFX", &[]);
        let t = sfx.remix_event_type;
        sfx.remix_post("RaceEventType", t as f32);
        self.run_ended = false;
        self.started = true;
    }

    pub fn race_start(&mut self, sfx: &mut SfxEngine) {
        sfx.remix_run(true);
    }

    pub fn shutdown(&mut self, sfx: &mut SfxEngine) {
        sfx.fire_event("trigger_STOP_Trick_SFX", &[]);
        sfx.remix_run(false);
        for h in [
            &mut self.board,
            &mut self.grind,
            &mut self.slide,
            &mut self.tumble,
            &mut self.awesome_global,
            &mut self.wingsuit,
            &mut self.ice_scrape,
            &mut self.big_air,
        ] {
            stop(sfx, h);
        }
        if let Some((h, _)) = self.sweet.take() {
            sfx.stop(h);
        }
        (self.axe_latch, self.axe_prev_blend) = (false, 0.0);
        self.started = false;
    }

    pub fn update(&mut self, sfx: &mut SfxEngine, r: &RideAudio) {
        self.remix.clear();
        let f = flags(r);
        self.mode = kind_mode(r.kind);
        let inputs = crate::sfx_bus::SnapshotInputs {
            mode: self.mode,
            nis: r.nis,
            nis_audio_type: r.nis_audio_type,
            inside_heli: r.inside_heli,
            eog: r.eog,
            deadly_descent: r.deadly_descent,
            kind: r.kind,
            flags: f,
            time_dilation: r.time_dilation,
            rewinding: r.rewinding,
            big_air_times: big_air_times(sfx),
            avalanche_mass: r.avalanche.mass,
            avalanche_distance: r.avalanche.distance,
            reverb: sfx.reverb,
            front_end: sfx.game_state != 0,
            wingsuit_audio: wingsuit_audio(sfx),
            script_song: sfx.script_song,
            tricky_clip: sfx.tricky.clip_playing(),
        };
        self.snapshots.frame(&mut sfx.mixer.graph, &inputs);
        if r.eog && !self.eog {
            sfx.results();
        }
        self.eog = r.eog;
        let s = SurfaceSummary::new(&r.weights);
        let spd = norm(r.speed, tuning::SPEED, 1000.0);
        let cls4 = s.class(0).clamp(1, 4);
        if !self.started {
            self.start(sfx, spd, cls4);
        }
        if r.event_ended_by_death {
            self.run_ended = true;
        }
        if r.jumped {
            if let Some(h) = call(sfx, "SFX_Jump", &[spd, cls4]) {
                sfx.stop(h);
            }
            self.remix.push(RemixEvent { name: "Jump", value: 0.0 });
        }
        self.board_and_grind(sfx, r, f, &s, spd);
        self.gear(sfx, r, f, spd, cls4);
        self.boost_and_awesome(sfx, r, f);
        self.time_dilation(sfx, r, f);
        let tumbling = bit(f, 12) && !bit(f, 13) && !self.run_ended;
        sfx.set_global("Is_Tumbling", i32::from(tumbling));
        sfx.set_global("Is_Sliding", i32::from(bit(f, 13) && !self.run_ended));
        self.big_air_sound(sfx, f);
        self.crash_and_tricks(sfx, r, f);
        self.flare_kick(sfx, r);
        self.ice_axe_message(sfx, r);
        self.module_update(sfx, r, spd, cls4);
        self.remix_events(r, f);
        for e in &self.remix {
            sfx.remix_post(e.name, e.value);
        }
        sfx.remix_frame(&crate::remix_runtime::FrameInputs {
            ms_in_air: r.ms_in_air,
            ms_to_land: r.ms_to_land,
            oxygen: r.vignettes.oxygen,
            temperature: r.vignettes.temperature,
            health: r.vignettes.health,
            rewinding: r.rewinding,
            dead_by_death_plane: r.dead_by_death_plane,
            ended_by_death: r.event_ended_by_death,
            dt_ms: 1000.0 / 30.0,
        });
        self.prev = f;
    }

    fn flare_kick(&mut self, sfx: &mut SfxEngine, r: &RideAudio) {
        let Some((kind, ty)) = r.trick_update else { return };
        if ty == 2 && kind == 3 && !self.flare_latch {
            if let Some(k) = sfx.aems.resolve(0x274F_2A01, "Board_or_body") {
                sfx.aems.set_global(k, 0);
            }
            let glance = (1.0 / tuning::OBJECT_COLLISION_GLANCE * 100.0).min(100.0) as i32;
            if let Some(k) = sfx.aems.resolve(0x274F_697B, "Hit_or_Glance") {
                sfx.aems.set_global(k, (100 - glance).clamp(0, 100));
            }
            let intensity = (r.speed / tuning::OBJECT_COLLISION_SPEED * 1000.0).min(1000.0) as i32;
            if let Some(k) = sfx.aems.resolve(0x08C9_398B, "Flare") {
                sfx.fire(k, &[intensity]);
            }
            self.flare_latch = true;
        } else {
            self.flare_latch &= ty == 2;
        }
    }

    fn board_and_grind(&mut self, sfx: &mut SfxEngine, r: &RideAudio, f: u32, s: &SurfaceSummary, spd: i32) {
        let cls = s.class(material_class(r.ground_material)).clamp(1, 8);
        let spray = norm(r.spray, tuning::SPRAY, 1000.0);
        let angle = (r.board_velocity_angle * 63.661_976).min(100.0) as i32;
        let lean = (if bit(f, 6) { 1 } else { -1 }) * (r.lean * 100.0) as i32;
        let nose_tail = (r.nose_tail * 100.0) as i32;
        let in_air = i32::from(!(bit(f, 0) && !bit(f, 14)));
        let grinding = (bit(f, 0) && bit(f, 14)) || bit(f, 21);
        sfx.set_global("Boarder_In_Air", in_air);
        sfx.set_global("Is_Grinding", i32::from(grinding));
        let material = if grinding {
            let m = if r.ground_material != 0 {
                r.ground_material
            } else {
                tuning::GRIND_MATERIAL.get(s.grind_class() as usize - 1).copied().unwrap_or(0)
            };
            match m {
                1..=19 => m,
                0x19 => 10,
                _ => 0,
            }
        } else {
            0
        };
        let board = [spd, cls, spray, angle.clamp(0, 100), lean.clamp(-100, 100), nose_tail.clamp(-100, 100), in_air];
        if let Some(h) = self.board {
            sfx.update(h, &board);
        }
        if let Some(h) = self.grind {
            sfx.update(h, &[spd, cls]);
        }
        if let Some((h, m)) = self.sweet {
            if m == material {
                sfx.update(h, &[spd, m]);
            } else {
                sfx.stop(h);
                self.sweet = None;
            }
        }
        if bit(f, 0) && !bit(self.prev, 0) {
            if self.board.is_none() {
                self.board = call(sfx, "SFX_Board", &board);
            }
            if self.grind.is_none() {
                self.grind = call(sfx, "SFX_Grind", &[spd, cls]);
            }
        }
        if grinding {
            if self.sweet.is_none() {
                self.sweet = call(sfx, "SFX_Grind_Sweetner", &[spd, material.clamp(1, 19)]).map(|h| (h, material));
            }
        } else if let Some((h, _)) = self.sweet.take() {
            sfx.stop(h);
        }
    }
}

impl RiderSounds {
    fn gear(&mut self, sfx: &mut SfxEngine, r: &RideAudio, f: u32, spd: i32, cls4: i32) {
        if bit(f, 3) {
            if !bit(self.prev, 3) && self.wingsuit.is_none() {
                self.wingsuit = call(sfx, "SFX_Wingsuit", &wingsuit_params(spd, r));
            }
        } else if bit(self.prev, 3) {
            stop(sfx, &mut self.wingsuit);
        }
        if bit(f, 15) {
            if self.ice_scrape.is_none() {
                if let Some(h) = call(sfx, "SFX_Ice_Axe_Impact", &[spd, cls4]) {
                    sfx.stop(h);
                }
                self.ice_scrape = call(sfx, "SFX_Ice_Axe_Scrape", &[spd, cls4]);
            }
        } else {
            stop(sfx, &mut self.ice_scrape);
        }
    }

    pub fn paused_frame(&mut self, sfx: &mut SfxEngine, r: &RideAudio) {
        let s = SurfaceSummary::new(&r.weights);
        let spd = norm(r.speed, tuning::SPEED, 1000.0);
        self.module_update(sfx, r, spd, s.class(0).clamp(1, 4));
    }

    fn module_update(&mut self, sfx: &mut SfxEngine, r: &RideAudio, spd: i32, cls4: i32) {
        if let Some(h) = self.slide {
            sfx.update(h, &[spd, cls4]);
        }
        if let Some(h) = self.tumble {
            sfx.update(h, &[spd, cls4, 1]);
        }
        if let Some(h) = self.wingsuit {
            sfx.update(h, &wingsuit_params(spd, r));
        }
        if let Some(h) = self.ice_scrape {
            sfx.update(h, &[spd, cls4]);
        }
    }

    fn ice_axe_message(&mut self, sfx: &mut SfxEngine, r: &RideAudio) {
        let w = r.ice_axe_blend;
        let one_shot = |sfx: &mut SfxEngine, name: &str| {
            if let Some(h) = call(sfx, name, &[]) {
                sfx.stop(h);
            }
        };
        if !self.axe_latch {
            if w > self.axe_prev_blend && w > 0.0 {
                self.axe_latch = true;
                one_shot(sfx, "SFX_Ice_Axe_Deploy");
            }
        } else if w < self.axe_prev_blend && w < 1.0 {
            self.axe_latch = false;
            one_shot(sfx, "SFX_Ice_Axe_Retract");
        }
        self.axe_prev_blend = w;
    }

    fn boost_and_awesome(&mut self, sfx: &mut SfxEngine, r: &RideAudio, f: u32) {
        let a = (r.boost_percent * 100.0) as i32;
        let b = (r.awesome_percent * 100.0) as i32;
        let d = [8, 9, 10, 11].iter().position(|&k| bit(f, k)).map_or(0, |k| k as i32 + 1);
        sfx.set_global("Boost_Percentage", a);
        sfx.set_global("Awesome_Percentage", b);
        sfx.set_global("Boost_in_Awesome_Percentage", a);
        let p = [a, b, 0, d];
        let (aw, bo, paw, pbo) = (bit(f, 4), bit(f, 5), bit(self.prev, 4), bit(self.prev, 5));
        if !aw && paw {
            sfx.fire_event("trigger_Stop_SFX_Awesome", &p);
        } else if aw && !paw {
            sfx.fire_event("trigger_SFX_Awesome", &p);
        }
        let mut stop_boost = false;
        if aw && bo && !(paw && pbo) && !self.awesome_boost_latch {
            sfx.fire_event("trigger_SFX_Awesome_Boost", &p);
            self.awesome_boost_latch = true;
            stop_boost = true;
        } else {
            let mut done = false;
            if (!aw || !bo) && paw && pbo && self.awesome_boost_latch {
                sfx.fire_event("trigger_Stop_SFX_Awesome_Boost", &p);
                self.awesome_boost_latch = false;
                done = true;
            }
            if !done {
                if bo {
                    if !pbo && !self.boost_latch && !self.awesome_boost_latch {
                        sfx.fire_event("trigger_SFX_Boost", &p);
                        self.boost_latch = true;
                    }
                } else if pbo {
                    stop_boost = true;
                }
            }
        }
        if stop_boost && self.boost_latch {
            sfx.fire_event("trigger_Stop_SFX_Boost", &p);
            self.boost_latch = false;
        }
        if bit(f, 28) {
            sfx.fire_event("trigger_SFX_Awesome_Landing", &p);
        }
        // PORT(choice): for music mods — with a client script's song `Tricky_State` is 2 only while "It's Tricky" plays
        // and 1 (Awesome) for the rest of Super Tricky (PR #53); 0x830EB7D8 keeps 2 for the whole of Super Tricky and
        // the game ships no script songs (the mod music contract).
        let tricky = if bit(f, 18) && (!sfx.script_song || sfx.tricky.clip_playing()) {
            2
        } else if bit(f, 18) || aw {
            1
        } else {
            0
        };
        sfx.set_global("Tricky_State", tricky);
    }

    pub(super) fn time_dilation(&mut self, sfx: &mut SfxEngine, r: &RideAudio, f: u32) {
        let td = r.time_dilation.clamp(0.0, tuning::TD_MAX);
        let speed = if td < 1.0 { (td * 100.0) as i32 - 100 } else { ((td - 1.0) / 3.0 * 100.0) as i32 };
        sfx.set_global("TimeDilation_Speed", speed);
        let a = (r.boost_percent * 100.0) as i32;
        let b = (r.awesome_percent * 100.0) as i32;
        let d = if bit(f, 9) { 2 } else { 0 };
        if self.dilating {
            if r.time_dilation > tuning::TD_STOP || !bit(f, 7) {
                sfx.fire_event("trigger_Stop_SFX_Awesome_TimeDialtion", &[a, b, 0, d]);
                self.dilating = false;
                sfx.set_global("Is_Time_Dilating", 0);
                sfx.set_global("Is_Time_Dilating_Boards", 0);
            }
        } else if bit(f, 7) && r.time_dilation < tuning::TD_START {
            sfx.fire_event("trigger_SFX_Awesome_TimeDialtion", &[a, b, 0, d]);
            sfx.set_global("Is_Time_Dilating", 1);
            sfx.set_global("Is_Time_Dilating_Boards", 1);
            self.dilating = true;
        }
    }
}

fn big_air_times(sfx: &SfxEngine) -> Option<(i32, i32)> {
    use crate::sfx::speech::vars::id;
    sfx.speech.as_ref().map(|sp| (sp.vars.get(id::RIDER_AIR_TIME_MS), sp.vars.get(id::RIDER_MILLISECONDS_TO_LANDING)))
}

impl RiderSounds {
    fn big_air_sound(&mut self, sfx: &mut SfxEngine, f: u32) {
        use crate::sfx::speech::vars::id;
        let race_active = sfx.speech.as_ref().is_some_and(|sp| sp.vars.get(id::RACE_IS_ACTIVE) == 1);
        let Some((air, land)) = big_air_times(sfx).filter(|_| race_active && !bit(f, 19)) else {
            stop(sfx, &mut self.big_air);
            return;
        };
        if air > tuning::BIG_AIR_MIN_AIR && land > tuning::BIG_AIR_START && self.big_air.is_none() {
            self.big_air = call(sfx, "SFX_Big_Air", &[land]);
        }
        if let Some(h) = self.big_air {
            if land < tuning::BIG_AIR_STOP || bit(f, 0) {
                stop(sfx, &mut self.big_air);
            } else {
                sfx.update(h, &[land.clamp(0, 10000)]);
            }
        }
    }

    fn crash_and_tricks(&mut self, sfx: &mut SfxEngine, r: &RideAudio, f: u32) {
        let crashing = bit(f, 12);
        if crashing && !self.crash {
            self.crash = true;
            if !r.sliding {
                self.remix.push(RemixEvent { name: "WipeoutStart", value: r.health_lost });
            }
        }
        if crashing && (r.board_particle || !self.impact_sent) {
            let h = if r.board_particle {
                call(sfx, "SFX_Body_Impact", &[])
            } else {
                let cls4 = SurfaceSummary::new(&r.weights).class(0).clamp(1, 4);
                self.impact_sent = true;
                call(sfx, "SFX_Board_Impact", &[norm(r.speed, tuning::CRASH, 1000.0), cls4])
            };
            if let Some(h) = h {
                sfx.stop(h);
            }
            sfx.fire_event("Stop_Signature_Trick", &[]);
            sfx.fire_event("Stop_Spin_Trick", &[]);
        }
        if !crashing && self.crash {
            self.crash = false;
            self.impact_sent = false;
            self.remix.push(RemixEvent { name: "WipeoutEnd", value: 0.0 });
        }
        if bit(f, 0) || bit(f, 3) {
            self.trick_latch = false;
        } else if r.tricking && bit(self.prev, 1) {
            self.trick_latch = true;
        }
        let (flip, spin) = if self.trick_latch {
            (norm(r.flip_speed.abs(), tuning::FLIP, 100.0), norm(r.spin_speed.abs(), tuning::SPIN, 100.0))
        } else {
            (0, 0)
        };
        sfx.set_global("Flip_Speed", flip);
        sfx.set_global("Spin_Speed", spin);
    }

    fn remix_events(&mut self, r: &RideAudio, f: u32) {
        let edge = |b: u32| (bit(f, b), bit(self.prev, b));
        let mut ev = |name: &'static str, value: f32| self.remix.push(RemixEvent { name, value });
        ev("UpdateCarve", if bit(f, 0) { r.lean } else { 0.0 });
        for (b, on, off) in [
            (3, "WingsuitDeployed", "WingsuitRetracted"),
            (7, "TimeDilationStart", "TimeDilationStop"),
            (5, "BoostStart", "BoostStop"),
            (12, "TumbleStart", "TumbleStop"),
            (18, "SuperAwesomeModeStart", "SuperAwesomeModeStop"),
            (28, "AwesomeLandStart", "AwesomeLandStop"),
            (14, "GrindingStart", "GrindingStop"),
        ] {
            match edge(b) {
                (true, false) => ev(on, 0.0),
                (false, true) => ev(off, 0.0),
                _ => {}
            }
        }
        match edge(4) {
            (true, false) => {
                ev("AwesomeModeStart", 0.0);
                ev("UpdatePitch", 1.0);
            }
            (false, true) => {
                ev("AwesomeModeStop", 0.0);
                ev("UpdatePitch", 1.0);
            }
            _ => {}
        }
        let tunnel = |x: u32| {
            if bit(x, 17) {
                2
            } else if bit(x, 16) {
                1
            } else {
                0
            }
        };
        match (tunnel(f), tunnel(self.prev)) {
            (t, 0) if t != 0 => ev("TunnelStart", t as f32),
            (0, t) if t != 0 => ev("TunnelStop", 0.0),
            _ => {}
        }
    }
}

fn wingsuit_audio(sfx: &SfxEngine) -> bool {
    match &sfx.launch_block {
        Some(b) => b.wingsuit_audio(),
        // PORT(choice): for hosts without a front-end launch (the asset viewer's ride, `sfx_ride`; port hosts only): no
        // game info block exists, so the wingsuit word counts as set and `WingSuit_1` follows bit 3 alone; the game
        // always builds the block (0x823F7788).
        None => true,
    }
}
