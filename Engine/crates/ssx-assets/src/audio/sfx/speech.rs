use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use ssx_formats::speech::{ctx::Contexts, evt::EventSystem, gas::GasFile, kwd::Keywords, rep::RepetitionPools, spc};

pub mod context;
pub mod game;
pub mod launch;
pub mod module;
pub mod pause;
pub mod vars;
pub mod vm;

use crate::career::random::{GameRandom, Source};
use context::{ContextSystem, Fire, Lcg, SpeechRandom};
use game::{SpeechGame, Trigger};
use module::{Bank, Line, SpeechModule};
use vars::Vars;

pub const RIDERS: [(&str, i32); 12] = [
    ("alex", 1),
    ("elise", 2),
    ("griff", 3),
    ("kaori", 4),
    ("mac", 5),
    ("moby", 6),
    ("psymon", 7),
    ("tane", 8),
    ("tyler", 9),
    ("zoe", 10),
    ("eddie", 11),
    ("travis", 12),
];

pub fn pilot_flag(pilot: i32) -> i32 {
    match pilot {
        1 => 1,
        2 => 2,
        3 => 4,
        _ => 0,
    }
}

pub fn helicopter_pilot(big_root: &Path, helicopter: u32) -> Result<(i32, String)> {
    let info = helicopter_info(big_root, helicopter)?;
    let id = info
        .iter()
        .find_map(|(k, v)| match v {
            HeliValue::Int(v) if *k == 0xF70C_CEC7 => Some(*v),
            _ => None,
        })
        .context("pilot: no speech id")?;
    let name = helicopter_text_of(&info, 0xB939_B740).context("pilot: no name")?;
    Ok((id, name.to_ascii_lowercase()))
}

pub fn helicopter_text(big_root: &Path, helicopter: u32, key: u32) -> Result<String> {
    let info = helicopter_info(big_root, helicopter)?;
    helicopter_text_of(&info, key).with_context(|| format!("helicopter: no text {key:#010x}"))
}

enum HeliValue {
    Int(i32),
    Text(String),
}

fn helicopter_text_of(info: &[(u32, HeliValue)], key: u32) -> Option<String> {
    info.iter().find_map(|(k, v)| match v {
        HeliValue::Text(t) if *k == key => Some(t.clone()),
        _ => None,
    })
}

fn helicopter_info(big_root: &Path, helicopter: u32) -> Result<Vec<(u32, HeliValue)>> {
    use crate::vault_db::{single, single_text};
    use ssx_formats::attrib::Value;
    crate::vault_db::with_all_db(big_root, |db| {
        let heli = db.fields(0x8136_C3AA, helicopter)?;
        let info = heli
            .iter()
            .find_map(|f| match single(&f.data) {
                Some(Value::Ref(r)) if f.key == 0x78A6_E89F => Some(r.collection),
                _ => None,
            })
            .context("helicopter: no pilot audio_game_info")?;
        let mut out = Vec::new();
        for f in db.fields(0x2306_91A5, info)? {
            if let Some(Value::Int32(v)) = single(&f.data) {
                out.push((f.key, HeliValue::Int(*v)));
            } else if let Some(t) = single_text(&f.data) {
                out.push((f.key, HeliValue::Text(t)));
            }
        }
        Ok(out)
    })
}

pub const DEFAULT_HELICOPTER: u32 = 0xB48A_C47B;

pub fn audio_helicopter(big_root: &Path, ui_drop: &str) -> Result<u32> {
    const RED_BULL_OWN: u32 = 0x6368_47C9;
    let c = crate::helicopter::drop_helicopter(big_root, ui_drop)?;
    if c.source == crate::helicopter::HeliSource::Drop && c.key == RED_BULL_OWN {
        return Ok(range_helicopter(big_root, c.range)?.unwrap_or(DEFAULT_HELICOPTER));
    }
    Ok(c.key)
}

fn range_helicopter(big_root: &Path, range: u32) -> Result<Option<u32>> {
    crate::vault_db::with_all_db(big_root, |db| {
        let heli = db
            .fields(crate::vault_db::fnv1("range"), range)?
            .into_iter()
            .find(|f| f.key == crate::vault_db::fnv1("helicopter"))
            .and_then(|f| crate::vault_db::refs(&f.data).first().map(|r| r.collection))
            .filter(|&k| k != 0);
        Ok(heli)
    })
}

pub fn drop_audio_game_info(big_root: &Path, drop_key: u32) -> Result<Option<[i32; 27]>> {
    use ssx_formats::attrib::{Schema, Value, Vault};
    const CLASS: u32 = 0x2306_91A5;
    let dir = big_root.join("bootcache/data/xenon/attribdb");
    let (sv, sb) = (std::fs::read(dir.join("attribdb_schema.vlt"))?, std::fs::read(dir.join("attribdb_schema.bin"))?);
    let schema = Schema::from_vault(&Vault::parse(&sv, &sb)?).context("reading the schema")?;
    let class = schema.class(CLASS).context("no audio_game_info class")?;
    crate::vault_db::with_all_db(big_root, |db| {
        if db.find(CLASS, drop_key).is_none() {
            return Ok(None);
        }
        let fields = db.fields(CLASS, drop_key)?;
        let mut words = [0i32; 27];
        for d in class.definitions.iter().filter(|d| d.in_layout() && usize::from(d.offset) < 27 * 4) {
            let value = fields.iter().find(|f| f.key == d.key).and_then(|f| crate::vault_db::single(&f.data));
            if let Some(Value::Int32(x)) = value {
                words[usize::from(d.offset) / 4] = *x;
            }
        }
        Ok(Some(words))
    })
}

fn drop_vars(v: &mut Vars, w: &[i32; 27]) {
    for (var, word) in [(91, 0x1A), (92, 0x10), (93, 0xF), (94, 0xE), (95, 0xD), (96, 10), (97, 5), (98, 3)]
        .into_iter()
        .chain([(99, 4), (100, 2), (191, 0xB), (192, 0xC), (193, 0), (194, 1)])
        .chain((0..8).map(|k| (310 + k, 0x19 - k)))
    {
        v.set(var, w[word as usize]);
    }
    let mut surface = 0;
    for (word, value) in [(9, 1), (7, 2), (8, 3), (6, 4)] {
        if w[word] != 0 {
            surface = value;
        }
    }
    v.set(195, surface);
}

#[derive(Clone, Debug)]
pub struct SpeechSetup {
    pub lang: String,
    pub rider: String,
    pub rider_id: i32,
    pub pilot_id: i32,
    pub pilot: String,
    pub region_id: i32,
    pub kind: game::EventKind,
    pub deadly_descent: bool,
    pub rng: SpeechRandom,
    pub random: Source,
    pub drop_info: Option<[i32; 27]>,
}

const VOICES: [(&str, &str); 6] = [
    ("Chopper_Speech_Voice", "chopper_speech_patch"),
    ("Rider_Speech_Voice", "rider_speech_patch"),
    ("RiderNPC1_Speech_Voice", "rider_npc1_speech_patch"),
    ("RiderNPC2_Speech_Voice", "rider_npc2_speech_patch"),
    ("RiderNPC3_Speech_Voice", "rider_npc3_speech_patch"),
    ("RiderNPC4_Speech_Voice", "rider_npc4_speech_patch"),
];

const FADE_OUT: f32 = 0.1;
const BUSY_TIME: f32 = 0.1;
const FREE_TIME: f32 = 0.5;

#[derive(Default)]
struct Voice {
    queue: VecDeque<(Queued, f32)>,
    busy: bool,
}

#[derive(Clone, Debug)]
enum Queued {
    Disc { bank: usize, sample: u32 },
    Package { draw: crate::sfx_override::Draw, gain: f32, id: String },
}

pub struct Speech {
    pub vars: Vars,
    pub contexts: ContextSystem,
    pub module: SpeechModule,
    pub game: SpeechGame,
    pub rng: SpeechRandom,
    random: Source,
    voices: Vec<Voice>,
    handles: Vec<Option<u32>>,
    fading: Vec<u32>,
    events: Vec<Trigger>,
    pub log: Vec<String>,
    time: f32,
    paused: bool,
    lang: String,
    overrides: HashMap<String, Arc<crate::sfx_override::OverrideSound>>,
}

impl Speech {
    pub fn load(big_root: &Path, setup: &SpeechSetup) -> Result<Self> {
        let (lang, rider) = (setup.lang.as_str(), setup.rider.as_str());
        let dir = format!("data/xenon/audio/speech/{lang}");
        let read = |rel: String| -> Result<Vec<u8>> {
            let p = crate::music::find_file(big_root, &rel).with_context(|| rel.clone())?;
            Ok(std::fs::read(p)?)
        };
        let ctx_evt = EventSystem::parse(&read(format!("{dir}/pilot_and_rider_contexteventsubscr_{lang}.bin"))?)?;
        let speech_evt = EventSystem::parse(&read(format!("{dir}/pilot_and_rider_eventsubscr_{lang}.bin"))?)?;
        let contexts = Contexts::parse(&read(format!("{dir}/pilot_and_rider_contexts_{lang}.bin"))?)?;
        let gas = GasFile::parse(&read(format!("{dir}/pilot_and_rider_contextgasscript_{lang}.gas"))?)?;
        let sentences = spc::SentenceData::parse(&read(format!("{dir}/pilot_and_rider_sentencedata_{lang}.bin"))?)?;
        let pools = RepetitionPools::parse(&read(format!("{dir}/pilot_and_rider_repetitionpools_{lang}.bin"))?)?;
        let keywords = Keywords::parse(&read(format!("{dir}/riders/{rider}_keywordsdata_{lang}.bin"))?)?;
        let xml =
            String::from_utf8_lossy(&read("data/xenon/audio/speech/pilot_and_rider_contextmoduledef.xml".into())?)
                .to_string();
        let root = ssx_formats::xml::parse(xml.trim_start_matches('\u{feff}'))?;
        let mut groups: Vec<Vec<String>> = Vec::new();
        for m in root.children_named("Module") {
            for g in m.children_named("InterruptGroup") {
                let i: usize = g.attr("groupIndex").and_then(|v| v.parse().ok()).unwrap_or(0);
                if groups.len() <= i {
                    groups.resize(i + 1, Vec::new());
                }
                groups[i].push(g.attr("voiceName").unwrap_or_default().to_string());
            }
        }
        let mut module = SpeechModule::new(speech_evt, sentences, pools, Some(keywords));
        module.banks.push(Bank::load(big_root, lang, "pilots", &setup.pilot)?);
        module.banks.push(Bank::load(big_root, lang, "riders", rider)?);
        let contexts = ContextSystem::new(contexts, gas, &ctx_evt, groups)?;
        let mut v = Vars::default();
        v.set(vars::id::RIDER_CHARACTER_ID, setup.rider_id);
        v.set(vars::id::CHOPPER_PILOT_CHARACTER_ID, setup.pilot_id);
        v.set(vars::id::REGION, setup.region_id);
        v.set(vars::id::IS_DEADLY_DESCENT, i32::from(setup.deadly_descent));
        let mut log = Vec::new();
        if let Some(w) = &setup.drop_info {
            drop_vars(&mut v, w);
            let set: Vec<String> = (91..=100)
                .chain(191..=195)
                .chain(310..=317)
                .filter(|&k| v.raw(k) != 0)
                .map(|k| format!("{k}={}", v.raw(k)))
                .collect();
            log.push(format!(
                "0.000 drop variables: {}",
                if set.is_empty() { "none set".into() } else { set.join(" ") }
            ));
        }
        Ok(Self {
            vars: v,
            contexts,
            module,
            game: SpeechGame::new(pilot_flag(setup.pilot_id), setup.rider_id, 2, setup.kind),
            rng: setup.rng.clone(),
            random: setup.random.clone(),
            voices: (0..VOICES.len()).map(|_| Voice::default()).collect(),
            handles: vec![None; VOICES.len()],
            fading: Vec::new(),
            events: Vec::new(),
            log,
            time: 0.0,
            paused: false,
            lang: lang.to_string(),
            overrides: HashMap::new(),
        })
    }

    fn now_ms(&self) -> u32 {
        (self.time * 1000.0) as u32
    }

    pub fn triggers(&mut self, list: Vec<Trigger>) {
        let now = self.now_ms();
        for t in list {
            let n = self.contexts.trigger(t.name, &t.params, now);
            self.log.push(format!("{:.3} {} {:?} → {n} contexts", self.time, t.name, t.params));
            if std::env::var("SSX_SPEECH_EXPLAIN").is_ok() {
                for (c, prio, fail) in self.contexts.explain(t.name, &t.params, &self.vars, now) {
                    self.log.push(format!(
                        "{:.3}   context {c} ({prio}): {}",
                        self.time,
                        fail.unwrap_or_else(|| "valid".into())
                    ));
                }
            }
        }
    }
}

const TRIGGERS: [&str; 27] = [
    "trigger_a_placeholder",
    "trigger_COLLISION",
    "trigger_DEATH",
    "trigger_DEATH_RIDER",
    "trigger_HAZARD_ENTERED",
    "trigger_HAZARD_ENTERED_RIDER",
    "trigger_HAZARD_EXITED",
    "trigger_HAZARD_EXITED_RIDER",
    "trigger_HAZARD_UPCOMING",
    "trigger_HELI_SKI_GRAB",
    "trigger_HELI_SKI_GRAB_RIDER",
    "trigger_INTRO",
    "trigger_RACE_PASSING_OPPONENT",
    "trigger_RACE_TIME_UPDATE",
    "trigger_RIDER_FALLING",
    "trigger_RIDER_LANDING",
    "trigger_RIDER_LANDING_RIDER",
    "trigger_RIDER_TUMBLING_END",
    "trigger_RIDER_TUMBLING_START",
    "trigger_RUN_END",
    "trigger_RUN_END_RIDER",
    "trigger_RUN_RESTART",
    "trigger_RUN_RESTART_RIDER",
    "trigger_RUN_START",
    "trigger_RUN_START_RIDER",
    "trigger_TRICK_GRINDING",
    "trigger_TUTORIAL",
];

impl Speech {
    pub fn frame(&mut self, f: &game::SpeechFrame, mixer: &mut crate::sfx_mixer::Mixer) {
        self.time = f.time;
        let events = std::mem::take(&mut self.events);
        let mut remaining = [0i32; 2];
        for v in 0..self.voices.len() {
            let mut left = self.handles[v].and_then(|h| mixer.pcm_remaining(h));
            if left.is_none() {
                self.handles[v] = None;
                if let Some((q, _)) = self.voices[v].queue.pop_front() {
                    self.start_sample(v, q, mixer);
                    left = self.handles[v].and_then(|h| mixer.pcm_remaining(h));
                }
            }
            let playing = left.is_some_and(|l| l > FREE_TIME) || !self.voices[v].queue.is_empty();
            if self.voices[v].busy && !playing {
                self.voices[v].busy = false;
                self.contexts.set_busy(VOICES[v].0, false);
                self.log.push(format!("{:.3} {} free", f.time, VOICES[v].0));
            }
            if v < 2 {
                let queued: f32 = self.voices[v].queue.iter().map(|q| q.1).sum();
                remaining[v] = ((left.unwrap_or(0.0) + queued) * 1000.0) as i32;
            }
        }
        let mut trig = events;
        let random = self.random.below(100) as i32;
        self.game.frame(&mut self.vars, f, (remaining[0], remaining[1]), random, &mut trig);
        self.triggers(trig);
        let now = self.now_ms();
        let (mut refire, mut lines, mut package) = (Vec::new(), Vec::new(), Vec::new());
        let module = &self.module;
        let overrides = &self.overrides;
        let contexts = &mut self.contexts;
        let vars = &self.vars;
        let fired = self.rng.with(|rng| {
            contexts.update(now, vars, rng, &mut |fire: &Fire, rng: &mut Lcg| {
                if let Some(&name) = TRIGGERS.iter().find(|n| **n == fire.event) {
                    refire.push(Trigger { name, params: fire.params.clone() });
                    return true;
                }
                if let Some(sound) = overrides.get(&fire.event.to_ascii_lowercase())
                    && let Some(draw) = sound.draw()
                {
                    package.push((fire.event.clone(), fire.params.first().copied().unwrap_or(0), sound.clone(), draw));
                    return true;
                }
                match module.choose(&fire.event, &fire.params, rng) {
                    Some(line) => {
                        lines.push(line);
                        true
                    }
                    None => false,
                }
            })
        });
        if let Some(fire) = &fired {
            self.log.push(format!("{:.3} context {} → {} {:?}", f.time, fire.context, fire.event, fire.params));
        }
        for line in lines {
            self.play(line, mixer);
        }
        for (event, voice, sound, draw) in package {
            let file = draw.sample.file.clone();
            let gain = sound.volume;
            let queued = Queued::Package { draw, gain, id: sound.id.clone() };
            self.log
                .push(format!("{:.3} {event}: the package line {} ({}) plays {file}", f.time, sound.id, sound.package));
            self.say(voice, vec![queued], mixer);
        }
        self.triggers(refire);
        self.reap(mixer);
    }

    pub fn pilot_speaking(&self) -> bool {
        self.voices[0].busy
    }

    pub fn set_launch(&mut self, b: &launch::LaunchBlock) {
        b.apply(&mut self.vars);
        (self.game.dd, self.game.dd_hold) = (b.campaign.dd, b.dd_hold);
        let w = &b.campaign;
        self.log.push(format!(
            "0.000 launch block: chapter {}/{} drop {}/{} dd {} hold {} before_dd {} completed {} intro {} counts {}/{}",
            w.chapter,
            w.chapters,
            w.drop,
            w.drops,
            w.dd,
            b.dd_hold,
            w.before_dd,
            w.completed,
            w.region_intro,
            w.completions,
            w.failures
        ));
    }

    pub fn event_setup(&mut self, time: f32, avalanche_probability: f32) {
        self.time = time;
        self.game.event_setup(&mut self.vars, time, avalanche_probability);
        self.log.push(format!(
            "{time:.3} event set-up: pilot {} rider {} kind {:?}",
            self.game.pilot, self.game.rider, self.game.kind
        ));
    }

    pub fn race_start(&mut self, time: f32, bots: i32) {
        self.game.race_start(&mut self.vars, time, bots, &mut self.events);
    }

    pub fn death(&mut self, c: &game::DeathCauses) {
        self.game.death(&mut self.vars, c, &mut self.events);
    }

    pub fn crash(&mut self, start: bool) {
        self.game.crash(&mut self.vars, start, &mut self.events);
    }

    pub fn trick_message(&mut self, time: f32, m: &game::TrickMsg) {
        self.game.trick_message(&mut self.vars, time, m);
    }

    pub fn run_end(&mut self, end: &ssx_rules::results::RunEnd, bots: i32) {
        self.game.run_end(&mut self.vars, end, bots, &mut self.events);
    }

    pub fn set_game_var(&mut self, var: i32, value: i32) {
        self.vars.set(var, value);
    }

    pub fn message(&mut self, pause: bool, mixer: &mut crate::sfx_mixer::Mixer) {
        self.paused = pause;
        for h in self.handles.iter().flatten().chain(&self.fading) {
            mixer.pause_pcm(*h, pause);
        }
        self.log.push(format!("{:.3} {}", self.time, if pause { "PauseSpeech" } else { "UnpauseSpeech" }));
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    pub fn playing(&self, mixer: &crate::sfx_mixer::Mixer) -> Vec<(&'static str, f32, bool)> {
        let mut out = Vec::new();
        for (v, h) in self.handles.iter().enumerate() {
            if let Some(h) = *h
                && let Some(left) = mixer.pcm_left(h)
            {
                out.push((VOICES[v].0, left, mixer.pcm_paused(h)));
            }
        }
        out
    }

    fn start_sample(&mut self, v: usize, q: Queued, mixer: &mut crate::sfx_mixer::Mixer) {
        match q {
            Queued::Disc { bank: b, sample: s } => {
                let bank = &self.module.banks[b];
                match bank.stream(s) {
                    Ok(sound) => {
                        let label = format!("{} sample {s:#x}", bank.name);
                        self.handles[v] = mixer.start_stream(VOICES[v].1, VOICES[v].0, sound, label);
                    }
                    Err(e) => self.log.push(format!("{:.3} {} sample {s:#x}: {e:#}", self.time, bank.name)),
                }
            }
            Queued::Package { draw, gain, id } => {
                let label = format!("{id} {}", draw.sample.file);
                self.handles[v] = mixer.start_package(VOICES[v].1, VOICES[v].0, draw.sample, draw.pitch, gain, label);
            }
        }
        if self.paused
            && let Some(h) = self.handles[v]
        {
            mixer.pause_pcm(h, true);
        }
    }

    pub fn set_overrides(
        &mut self,
        all: &std::collections::HashMap<(String, String), Arc<crate::sfx_override::OverrideSound>>,
    ) {
        self.overrides = all
            .iter()
            .filter(|((lang, _), _)| *lang == self.lang)
            .map(|((_, event), s)| (event.to_ascii_lowercase(), s.clone()))
            .collect();
    }

    fn play(&mut self, line: Line, mixer: &mut crate::sfx_mixer::Mixer) {
        let v = usize::try_from(line.voice).unwrap_or(0).min(self.voices.len() - 1);
        self.module.played(&line);
        let names: Vec<String> =
            line.samples.iter().map(|&(b, s, _)| format!("{}:{s:#x}", self.module.banks[b].name)).collect();
        self.log.push(format!("{:.3} {} line {} [{}]", self.time, VOICES[v].0, line.event, names.join(", ")));
        let samples = line.samples.iter().map(|&(bank, sample, _)| Queued::Disc { bank, sample }).collect();
        self.say(line.voice, samples, mixer);
    }

    fn say(&mut self, voice: i32, samples: Vec<Queued>, mixer: &mut crate::sfx_mixer::Mixer) {
        let v = usize::try_from(voice).unwrap_or(0).min(self.voices.len() - 1);
        let current = self.handles[v].and_then(|h| mixer.pcm_remaining(h));
        if current.is_none() {
            self.handles[v] = None;
        }
        if (current.is_some_and(|l| l > BUSY_TIME) || !self.voices[v].queue.is_empty())
            && let Some(h) = self.handles[v].take()
        {
            mixer.fade_pcm(h, FADE_OUT);
            self.fading.push(h);
        }
        let banks = &self.module.banks;
        self.voices[v].queue = samples
            .into_iter()
            .map(|q| {
                let secs = match &q {
                    Queued::Disc { bank, sample } => banks[*bank].seconds(*sample),
                    Queued::Package { draw, .. } if draw.sample.frames() > 100 => {
                        draw.sample.frames() as f32 / crate::sfx_aems::MIXER_RATE as f32
                    }
                    Queued::Package { .. } => 0.0,
                };
                (q, secs)
            })
            .collect();
        if self.handles[v].is_none()
            && let Some((q, _)) = self.voices[v].queue.pop_front()
        {
            self.start_sample(v, q, mixer);
        }
        if !self.voices[v].busy {
            self.voices[v].busy = true;
            self.contexts.set_busy(VOICES[v].0, true);
        }
    }

    fn reap(&mut self, mixer: &mut crate::sfx_mixer::Mixer) {
        self.fading.retain(|&h| mixer.pcm_remaining(h).is_some());
    }
}
