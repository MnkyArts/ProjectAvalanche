use std::path::Path;

use anyhow::{Context, Result};

use crate::sfx_aems::{Aems, Bank, CsisHandle, FRAME_SAMPLES, tick_frames};
use crate::sfx_mixer::Mixer;
use crate::sfx_tuna::{CallKind, Tuna};

pub use banks::*;
pub use mix::*;

mod banks;
pub mod frontend;
mod frontend_tables;
pub mod heli;
mod mix;
pub mod movie;
pub mod music_pause;
pub mod nis;
pub mod output;
pub mod settings;
pub mod speech;
pub mod store;
pub mod tricky;

pub const SCRIPT_BUSES: [&str; 8] = [
    "Script_3D_0_BUS",
    "Script_3D_1_BUS",
    "Script_3D_2_BUS",
    "Script_3D_3_BUS",
    "Script_3D_4_BUS",
    "Script_3D_5_BUS",
    "Script_3D_6_BUS",
    "Script_3D_7_BUS",
];
pub const SCRIPT_ORIGIN: u32 = 0xFFFF_FF00;
// TODO(guess): a script's positioned sound is heard with the board centre's 3-D rules (`boarder_board_center`: rolloff
// 0.0115, 1–200 m, cut at 200 m, centre level 0.5) at the script's point (its offset 0): the game has no script sounds;
// settle: an `audio_3d_settings` collection a package names (M8's gamemodes), or measured with real packages.
pub const SCRIPT_EMITTER: crate::sfx_dsp::sound3d::EmitterSettings =
    crate::sfx_dsp::sound3d::EmitterSettings { offset: [0.0; 3], ..crate::sfx_dsp::sound3d::BOARD_CENTER };

pub struct SfxEngine {
    pub aems: Aems,
    pub mixer: Mixer,
    pub tuna: Tuna,
    pub remix: Option<crate::remix_runtime::Remix>,
    pub remix_event_type: i32,
    until_tick: usize,
    tick_blocks: usize,
    since_update: f32,
    pace: f32,
    remix_clock: f32,
    audio_ms: f64,
    update_period: f32,
    block6: Vec<f32>,
    block: Vec<f32>,
    used: usize,
    pub frames: u64,
    pub reverb: bool,
    pub sound3d: crate::sfx_dsp::sound3d::Renderer,
    pub speech: Option<speech::Speech>,
    pub ui_goals: [i32; 2],
    pub launch_block: Option<speech::launch::LaunchBlock>,
    pub settings: settings::AudioSettings,
    pub ranges: settings::FeRanges,
    faders: settings::FeFaders,
    fe_gain: [f32; 5],
    fe_trim: [f32; 5],
    pub tricky: tricky::TrickySong,
    tricky_sample_end: Option<u32>,
    pub game_state: i32,
    pub game_result: u8,
    pub tutorial: bool,
    speech_overrides: std::collections::HashMap<(String, String), std::sync::Arc<crate::sfx_override::OverrideSound>>,
}

pub const UPDATE_PERIOD: f32 = 1.0 / 60.0;

pub const PAUSE_CLASSES: [&str; 8] =
    ["Ant_Tags", "Helicopter", "Tricks", "Avalanche", "Awesome", "Environment", "Gear", "Boards"];

impl SfxEngine {
    pub fn load(big_root: &Path, banks: &[&str]) -> Result<Self> {
        let mut aems = Aems::new();
        aems.load_interfaces(big_root)?;
        let set = std::sync::Arc::new(crate::sfx_bus::PatchSet::load(big_root)?);
        let table = crate::sfx_mixer::voice_patch_table(big_root)?;
        let mut mixer = Mixer::new(set, table);
        for name in banks {
            let bank = Bank::load(big_root, name)?;
            mixer.add_bank(&bank);
            aems.add_bank(bank);
        }
        let tuna = Tuna::load(big_root, &aems)?;
        let ranges = settings::FeRanges::load(big_root)?;
        let mut settings = settings::AudioSettings::defaults(&ranges);
        if std::env::var("SSX_MUSIC_REMIX").is_ok_and(|v| matches!(v.as_str(), "off" | "0")) {
            settings.0[settings::MUSIC_REMIX] = 0;
        }
        let remix = match crate::remix_runtime::Remix::load(big_root, &mut mixer.graph) {
            Ok(mut r) => {
                r.remix_setting = settings.get(settings::MUSIC_REMIX) != 0;
                Some(r)
            }
            Err(e) => {
                ssx_core::eprintln!("warning: music remix unavailable: {e:#}");
                None
            }
        };
        let fe_gain = settings::FADERS.map(|(bus, _)| {
            mixer.graph.attrs_by_name(bus, b"GaF0").and_then(|a| a.first().copied()).unwrap_or(1.0) as f32
        });
        let tricky_sample_end = aems.resolve(tricky::TRICKY_SAMPLE_END.0, tricky::TRICKY_SAMPLE_END.1);
        if let Some(k) = tricky_sample_end {
            aems.watch_global(k);
        }
        let tricky = tricky::TrickySong::new(settings.get(settings::TRICKY_STATE));
        let (n, _) = tick_frames();
        let block = vec![0.0; 2 * FRAME_SAMPLES];
        Ok(Self {
            aems,
            mixer,
            tuna,
            remix,
            remix_event_type: 1,
            until_tick: 0,
            tick_blocks: n as usize,
            since_update: UPDATE_PERIOD,
            pace: UPDATE_PERIOD,
            audio_ms: 0.0,
            remix_clock: crate::remix::TICK,
            update_period: UPDATE_PERIOD,
            block6: vec![0.0; crate::sfx_bus::DAC_CHANNELS * FRAME_SAMPLES],
            block,
            used: FRAME_SAMPLES,
            frames: 0,
            reverb: false,
            sound3d: Default::default(),
            speech: None,
            ui_goals: [0; 2],
            launch_block: None,
            settings,
            ranges,
            faders: settings::FeFaders::default(),
            fe_gain,
            fe_trim: [1.0; 5],
            tricky,
            tricky_sample_end,
            game_state: 0,
            game_result: 0,
            tutorial: false,
            speech_overrides: std::collections::HashMap::new(),
        })
    }

    pub fn apply_settings(&mut self, s: &settings::AudioSettings) {
        self.settings = *s;
    }

    pub fn set_loop_rate(&mut self, hz: f32) {
        self.update_period = 1.0 / hz.max(1.0);
    }

    pub fn set_trims(&mut self, sfx: f32, music: f32) {
        let trim = |bus: &str| match bus {
            "FE_Setting_SFX" => sfx,
            "FE_Setting_Music" => music,
            _ => 1.0,
        };
        for (k, (bus, _)) in settings::FADERS.iter().enumerate() {
            let t = trim(bus);
            if t != self.fe_trim[k] {
                self.fe_trim[k] = t;
                self.mixer.graph.set_attr_by_name(bus, b"GaF0", 0, f64::from(self.fe_gain[k] * t));
            }
        }
    }

    pub fn set_game_state(&mut self, state: i32) {
        self.game_state = state;
        self.tricky.game_state(state);
    }

    pub fn game_pause(&mut self, paused: bool) {
        for class in PAUSE_CLASSES {
            self.set_global(&format!("{class}_send_pause_on"), if paused { 2 } else { 1 });
        }
    }

    pub fn speech_message(&mut self, pause: bool) {
        if let Some(s) = self.speech.as_mut() {
            s.message(pause, &mut self.mixer);
        }
    }

    pub fn results(&mut self) {
        self.tricky.results(&mut self.aems);
    }

    pub fn add_bank(&mut self, big_root: &Path, name: &str) -> Result<()> {
        let bank = Bank::load(big_root, name)?;
        self.mixer.add_bank(&bank);
        self.aems.add_bank(bank);
        Ok(())
    }

    pub fn add_loaded_bank(&mut self, bank: Bank, ram: crate::sfx_mixer::DecodedRam) {
        self.mixer.add_bank_decoded(&bank, ram);
        self.aems.add_bank(bank);
    }

    pub fn decode_ram_sounds(&mut self, threads: usize) -> Result<(usize, usize)> {
        self.mixer.decode_ram_all(threads)?;
        let (n, floats) = self.mixer.decoded_ram();
        Ok((n, floats * 4))
    }

    pub fn load_speech(&mut self, big_root: &Path, setup: &speech::SpeechSetup) -> Result<()> {
        self.set_speech(speech::Speech::load(big_root, setup)?);
        Ok(())
    }

    pub fn set_speech(&mut self, mut speech: speech::Speech) {
        speech.game.goals = self.ui_goals;
        if let Some(b) = &self.launch_block {
            speech.set_launch(b);
        }
        if !self.speech_overrides.is_empty() {
            speech.set_overrides(&self.speech_overrides);
        }
        self.speech = Some(speech);
    }

    pub fn speech_frame(&mut self, f: &speech::game::SpeechFrame) {
        if let Some(s) = self.speech.as_mut() {
            s.frame(f, &mut self.mixer);
        }
    }

    pub fn sound3d_frame(
        &mut self,
        camera: Option<([f32; 3], [f32; 3])>,
        emitters: &[crate::sfx_dsp::sound3d::Emitter],
        dt: f32,
    ) {
        for (bus, p) in self.sound3d.frame(camera, emitters, dt) {
            self.mixer.graph.pan3d_send(bus, Some(&p));
        }
    }

    pub fn sound3d_frame_scripts(
        &mut self,
        camera: Option<([f32; 3], [f32; 3])>,
        emitters: &[crate::sfx_dsp::sound3d::Emitter],
        dt: f32,
    ) {
        for (bus, p) in self.sound3d.frame_scripts(camera, emitters, dt) {
            self.mixer.graph.pan3d_send(bus, Some(&p));
        }
    }

    pub fn start_reverb(&mut self, big_root: &Path, info: &TrackAudioInfo) -> Result<()> {
        for (slot, name) in [(0, &info.reverb_small), (1, &info.reverb_large)] {
            let rel = format!("data/xenon/audio/reverb/{name}.irf");
            let path = crate::music::find_file(big_root, &rel).with_context(|| rel.clone())?;
            let irf = ssx_formats::irf::Irf::parse(&std::fs::read(&path)?).with_context(|| rel.clone())?;
            let ir = std::sync::Arc::new(crate::sfx_dsp::reverb_ir::ImpulseResponse::new(&irf));
            if !self.mixer.graph.reverb_apply("SFX_Wet", slot, ir) {
                anyhow::bail!("no ReverbIR1 on SFX_Wet");
            }
        }
        self.reverb = true;
        Ok(())
    }
    pub fn set_overrides(&mut self, o: &crate::sfx_override::SoundOverrides) -> Vec<String> {
        if self.aems.voice_names.is_empty() {
            self.aems.voice_names = self.mixer.voice_table().clone();
        }
        let mut by_key = std::collections::HashMap::new();
        let mut missing = Vec::new();
        for ((file, name), sound) in &o.effects {
            match self.aems.key_in(file, name) {
                Some(k) => {
                    by_key.insert(k, sound.clone());
                }
                None => missing.push(sound.id.clone()),
            }
        }
        self.aems.set_overrides(by_key);
        self.speech_overrides = o.speech.clone();
        if let Some(sp) = self.speech.as_mut() {
            sp.set_overrides(&self.speech_overrides);
        }
        missing
    }

    pub fn set_verbose(&mut self, on: bool) {
        self.aems.verbose = on;
        self.mixer.verbose = on;
    }

    pub fn call(&mut self, function: &str, params: &[i32]) -> Option<CsisHandle> {
        let id = self.aems.id(function)?;
        Some(self.aems.call(id, params))
    }

    pub fn call_live(&mut self, function: &str, params: &[i32]) -> Option<CsisHandle> {
        let h = self.call(function, params)?;
        (!self.aems.discard_if_empty(h)).then_some(h)
    }

    pub fn call_key(&mut self, key: u32, params: &[i32]) -> CsisHandle {
        self.aems.call(key, params)
    }

    pub fn update(&mut self, handle: CsisHandle, params: &[i32]) {
        self.aems.update(handle, params);
    }

    pub fn stop(&mut self, handle: CsisHandle) {
        self.aems.stop(handle);
    }

    pub fn set_global(&mut self, name: &str, value: i32) {
        if let Some(id) = self.aems.id(name) {
            self.aems.set_global(id, value);
        }
    }

    pub fn fire_event(&mut self, name: &str, params: &[i32]) {
        if let Some(key) = self.aems.id(name) {
            self.fire(key, params);
        }
    }

    pub fn fire(&mut self, key: u32, params: &[i32]) {
        self.fire_with(key, params, None);
    }

    pub fn fire_from(&mut self, key: u32, params: &[i32], origin: u32) {
        self.fire_with(key, params, Some(origin));
    }

    pub fn script_buses(&mut self) -> usize {
        let n = self.mixer.graph.add_bus_copies("Board_Center_BUS", &SCRIPT_BUSES);
        for (k, bus) in SCRIPT_BUSES.iter().enumerate().take(n) {
            self.aems.script_routes.insert(SCRIPT_ORIGIN + k as u32, (*bus).to_string());
        }
        n
    }

    pub fn bus_senders(&self, bus: &str) -> usize {
        self.mixer.graph.submix_input(bus).map_or(0, |i| self.mixer.graph.senders_into(i))
    }

    fn fire_with(&mut self, key: u32, params: &[i32], origin: Option<u32>) {
        self.aems.fire_event_from(key, params, origin);
        let Some(routes) = self.tuna.routes.get(&key).cloned() else { return };
        let name = self.aems.name(key);
        self.aems.log.push(crate::sfx_aems::LogLine { ms: self.aems.now_ms, text: format!("tuna {name} {params:?}") });
        for r in routes {
            let Some(t) = r.target else { continue };
            let p = r.params(params);
            match r.kind {
                CallKind::Function => {
                    let h = self.aems.call_from(t, &p, Some(origin.unwrap_or(key)));
                    self.aems.stop(h);
                }
                CallKind::Event => self.aems.fire_event_from(t, &p, Some(origin.unwrap_or(key))),
                CallKind::Global => {
                    let v = p.first().copied().unwrap_or(0);
                    self.aems.set_global(t, v);
                }
            }
        }
    }

    pub fn remix_post(&mut self, name: &str, value: f32) {
        if let Some(r) = self.remix.as_mut() {
            r.inputs.post(name, value);
        }
        let setting = self.settings.get(settings::TRICKY_STATE);
        self.tricky.post(&mut self.aems, name, value, setting);
    }

    pub fn remix_frame(&mut self, f: &crate::remix_runtime::FrameInputs) {
        self.tricky.rewind(&mut self.aems, f.rewinding);
        if let Some(r) = self.remix.as_mut() {
            r.inputs.frame(f);
            self.tricky.enabled = r.inputs.enabled;
        }
    }

    pub fn remix_run(&mut self, on: bool) {
        if let Some(r) = self.remix.as_mut() {
            r.inputs.enabled = on;
            r.inputs.armed = on;
        }
        self.tricky.enabled = on;
    }

    pub fn render(&mut self, out: &mut [f32]) {
        self.render_layout(out, 2);
    }

    pub fn render_dac(&mut self, out: &mut [f32]) {
        self.render_layout(out, crate::sfx_bus::DAC_CHANNELS);
    }

    fn render_layout(&mut self, out: &mut [f32], channels: usize) {
        let n = out.len() / channels;
        let mut done = 0;
        while done < n {
            if self.used == FRAME_SAMPLES {
                self.next_block();
            }
            let take = (n - done).min(FRAME_SAMPLES - self.used);
            let block = if channels == 2 { &self.block } else { &self.block6 };
            let src = &block[self.used * channels..];
            for (o, x) in out[done * channels..(done + take) * channels].iter_mut().zip(src) {
                *o += x;
            }
            self.used += take;
            done += take;
        }
        self.frames += n as u64;
    }

    fn next_block(&mut self) {
        let prof = self.mixer.graph.profiling().then(std::time::Instant::now);
        if self.until_tick == 0 {
            self.aems.tick(&mut self.mixer);
            for (key, params) in self.aems.take_fired() {
                if let Some(routes) = self.tuna.routes.get(&key).cloned() {
                    for r in routes.into_iter().filter(|r| r.kind == CallKind::Event) {
                        if let Some(t) = r.target {
                            self.aems.fire_event_from(t, &r.params(&params), Some(key));
                        }
                    }
                }
            }
            self.until_tick = self.tick_blocks;
        }
        self.until_tick -= 1;
        let prof = prof.map(|t| {
            self.mixer.graph.profile_add("(aems tick)", t.elapsed().as_nanos() as u64);
            std::time::Instant::now()
        });
        if self.pace >= self.update_period {
            self.audio_manager_update();
            self.remix_clock += self.since_update;
            if self.remix_clock >= crate::remix::TICK {
                self.remix_clock = (self.remix_clock - crate::remix::TICK).min(crate::remix::TICK);
                if let Some(r) = self.remix.as_mut() {
                    r.tick(&mut self.mixer.graph);
                }
            }
            self.mixer.graph.controller_update(self.since_update);
            self.since_update = 0.0;
            self.pace = (self.pace - self.update_period).min(self.update_period);
        }
        let block = FRAME_SAMPLES as f32 / crate::sfx_aems::MIXER_RATE as f32;
        self.since_update += block;
        self.pace += block;
        self.audio_ms += FRAME_SAMPLES as f64 * 1000.0 / f64::from(crate::sfx_aems::MIXER_RATE);
        let ms = self.aems.now_ms;
        for text in self.mixer.graph.log.drain(..) {
            self.aems.log.push(crate::sfx_aems::LogLine { ms, text });
        }
        if let Some(t) = prof {
            self.mixer.graph.profile_add("(updates)", t.elapsed().as_nanos() as u64);
        }
        self.mixer.release_ended();
        self.mixer.process_block_dac(&mut self.block6);
        self.block.fill(0.0);
        crate::sfx_bus::fold_interleaved(&self.block6, &mut self.block);
        self.used = 0;
    }
}

impl SfxEngine {
    fn audio_manager_update(&mut self) {
        if let Some(r) = self.remix.as_mut() {
            r.inputs.events_frame(self.audio_ms);
            if std::mem::take(&mut r.inputs.reset) {
                self.tricky.reset(&mut self.aems);
            }
            self.tricky.enabled = r.inputs.enabled;
        }
        for (id, v) in self.aems.take_global_changes() {
            if Some(id) == self.tricky_sample_end {
                self.tricky.sample_end(v);
            }
        }
        let (tricky, menu_sfx) =
            (self.settings.get(settings::TRICKY_STATE), self.settings.get(settings::MENU_SFX_VOLUME));
        let ducker = self.tricky.frame(&mut self.aems, tricky, menu_sfx);
        if let Some(r) = self.remix.as_mut() {
            if let Some(on) = ducker {
                r.inputs.music_ducker(on);
            }
            r.remix_setting = self.settings.get(settings::MUSIC_REMIX) != 0;
        }
        let ms = self.aems.now_ms;
        for text in self.tricky.log.drain(..) {
            self.aems.log.push(crate::sfx_aems::LogLine { ms, text: format!("tricky song: {text}") });
        }
        if let Some(on) = ducker {
            let text = format!("remix Music Ducker {}", if on { "Start" } else { "Stop" });
            self.aems.log.push(crate::sfx_aems::LogLine { ms, text });
        }
        for (bus, target) in self.faders.update(&self.settings, &self.ranges) {
            let k = settings::FADERS.iter().position(|(b, _)| *b == bus).unwrap_or(0);
            self.fe_gain[k] = target;
            self.mixer.graph.fade_by_name(bus, target * self.fe_trim[k], settings::FADE_SECONDS);
        }
        let g = &mut self.mixer.graph;
        g.selectors[0].set(1, u8::from(self.settings.get(settings::SPEAKER_TYPE) == 0));
        let st = self.game_state;
        g.selectors[1].set(12, u8::from(st == frontend::state::TRANSITION));
        g.selectors[1].set(13, u8::from(st == frontend::state::PAUSE));
        g.selectors[1].set(14, u8::from(st == frontend::state::STAGING));
        g.selectors[2].set(26, u8::from(st == frontend::state::TRANSITION));
        if st != 0 {
            let on = |c: bool| u8::from(c);
            let game = matches!(st, 0xA..=0x10);
            for (k, s) in [(1, 0xB), (2, 0xC), (3, 0xD)] {
                g.selectors[1].set(k, on(st == s));
            }
            g.selectors[2].set(1, on(st == 0xB));
            g.selectors[2].set(2, on(st == 0xC || (st == 0xD && self.tutorial)));
            g.selectors[2].set(3, on(st == 0xD && !self.tutorial));
            if !game {
                g.selectors[1].set(4, 0);
                g.selectors[2].set(0, 2);
            }
            if st == 0xF {
                g.selectors[2].set(25, 1);
            }
            g.selectors[2].set(31, on(self.game_result == 1));
            g.selectors[2].set(32, on(matches!(self.game_result, 2 | 3)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Level(f32, Arc<AtomicBool>);

    impl MusicSource for Level {
        fn render(&mut self, out: &mut [f32]) {
            out.fill(self.0);
        }
        fn finished(&self) -> bool {
            self.1.load(Ordering::Relaxed)
        }
    }

    #[test]
    fn the_front_ends_music_sounds_with_the_levels_until_it_is_over() {
        let (never, done) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let mut mix = OutputMix::new(Some(Box::new(Level(0.25, never))), 44_100, None);
        mix.music_tail = Some(Box::new(Level(0.5, done.clone())));
        let mut out = vec![0f32; 2 * 4800];
        for _ in 0..4 {
            mix.render(&mut out);
        }
        let mid = out[out.len() / 2];
        assert!((mid - 0.75).abs() < 1e-3, "the level's music and the tail summed: {mid}");
        done.store(true, Ordering::Relaxed);
        mix.render(&mut out);
        assert!(mix.music_tail.is_none(), "a finished tail is dropped");
        for _ in 0..4 {
            mix.render(&mut out);
        }
        let mid = out[out.len() / 2];
        assert!((mid - 0.25).abs() < 1e-3, "the level's music alone: {mid}");
    }
}
