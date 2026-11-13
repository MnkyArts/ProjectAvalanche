use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use bevy::prelude::*;
use ssx_assets::sfx::{SCRIPT_BUSES, SCRIPT_EMITTER, SCRIPT_ORIGIN};
use ssx_assets::sfx_dsp::sound3d::Emitter;
use ssx_assets::sfx_override::OverrideSound;
use ssx_content::{BASE_NAMESPACE, ContentId};
use ssx_script::host::game::SoundOrder;

use crate::audio::music::GameAudio;

const MAX_HELD: usize = 64;
const MAX_HELD_BYTES: usize = 128 << 20;
const MAX_WAITING: usize = 64;
const MAX_DECODING: usize = 4;
const VOICE: &str = "FE_Button_SFX_Voice";
// PORT(choice): 32 package voices of the scripts at once (the host's 32 sounds a frame), a port bound; tuning: the
// mixer's voice budget measured with M8's packages; no game counterpart (the original runs no scripts).
const MAX_VOICES: usize = 32;

type Decoded = (ContentId, Result<Arc<OverrideSound>, String>);

const BUS_HOLD: f32 = 0.5;

#[derive(Clone, Copy)]
struct Placed {
    position: [f32; 3],
    since: f32,
}

#[derive(Resource)]
pub(super) struct ScriptSounds {
    pending: VecDeque<SoundOrder>,
    held: BTreeMap<ContentId, Arc<OverrideSound>>,
    order: VecDeque<ContentId>,
    bytes: usize,
    decoding: BTreeMap<ContentId, ()>,
    failed: BTreeMap<ContentId, String>,
    tx: Sender<Decoded>,
    rx: Mutex<Receiver<Decoded>>,
    pub(super) content: Option<Arc<ssx_assets::content::GameContent>>,
    played: BTreeMap<ContentId, u64>,
    buses: [Option<Placed>; SCRIPT_BUSES.len()],
    clock: f32,
    pub(super) refused_3d: u64,
    refused_voices: u64,
}

const LOGGED_PLAYS: u64 = 8;

impl ScriptSounds {
    fn logged(&mut self, o: &SoundOrder) {
        let n = self.played.entry(o.id.clone()).or_insert(0);
        *n += 1;
        if *n <= LOGGED_PLAYS {
            info!(
                "scripts: [{}] played {} ({}{})",
                o.package,
                o.id,
                *n,
                if *n == LOGGED_PLAYS { ", the last logged" } else { "" }
            );
        }
    }
}

impl Default for ScriptSounds {
    fn default() -> Self {
        let (tx, rx) = channel();
        ScriptSounds {
            pending: VecDeque::new(),
            held: BTreeMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            decoding: BTreeMap::new(),
            failed: BTreeMap::new(),
            tx,
            rx: Mutex::new(rx),
            content: None,
            played: BTreeMap::new(),
            buses: [None; SCRIPT_BUSES.len()],
            clock: 0.0,
            refused_3d: 0,
            refused_voices: 0,
        }
    }
}

impl ScriptSounds {
    pub(super) fn push(&mut self, orders: Vec<SoundOrder>) {
        for o in orders {
            if self.pending.len() == MAX_WAITING {
                self.pending.pop_front();
            }
            self.pending.push_back(o);
        }
    }

    pub(super) fn clear(&mut self) {
        self.pending.clear();
    }

    #[cfg(test)]
    pub(super) fn waiting(&self) -> usize {
        self.pending.len()
    }

    pub(super) fn preload(&mut self, id: ContentId) {
        if id.namespace() == BASE_NAMESPACE || self.held.contains_key(&id) {
            return;
        }
        self.decode(&id);
    }

    fn decode(&mut self, id: &ContentId) {
        if self.decoding.contains_key(id) || self.decoding.len() >= MAX_DECODING || self.failed.contains_key(id) {
            return;
        }
        let Some(content) = self.content.clone() else { return };
        let (tx, for_thread) = (self.tx.clone(), id.clone());
        let spawned = std::thread::Builder::new().name("script-sound".into()).spawn(move || {
            let r = content.script_sound(&for_thread);
            let _ = tx.send((for_thread, r));
        });
        match spawned {
            Ok(_) => _ = self.decoding.insert(id.clone(), ()),
            Err(e) => {
                let why = format!("no sound decoder thread ({e})");
                warn!("scripts: the sound {id} cannot play: {why}");
                self.failed.insert(id.clone(), why);
            }
        }
    }

    fn keep(&mut self, id: ContentId, s: Arc<OverrideSound>) {
        let size: usize = s.set.iter().map(|x| x.samples.len() * 4).sum();
        while (self.held.len() >= MAX_HELD || self.bytes + size > MAX_HELD_BYTES) && !self.order.is_empty() {
            if let Some(old) = self.order.pop_front()
                && let Some(o) = self.held.remove(&old)
            {
                self.bytes -= o.set.iter().map(|x| x.samples.len() * 4).sum::<usize>();
            }
        }
        if size <= MAX_HELD_BYTES {
            self.bytes += size;
            self.order.push_back(id.clone());
            self.held.insert(id, s);
        }
    }
}

impl ScriptSounds {
    fn take_bus(&mut self, senders: impl Fn(&str) -> usize, position: [f32; 3]) -> Option<usize> {
        let now = self.clock;
        let free = (0..SCRIPT_BUSES.len()).find(|&k| match self.buses[k] {
            None => true,
            Some(p) => now - p.since >= BUS_HOLD && senders(SCRIPT_BUSES[k]) == 0,
        });
        match free {
            Some(k) => {
                self.buses[k] = Some(Placed { position, since: now });
                Some(k)
            }
            None => {
                self.refused_3d += 1;
                if self.refused_3d == 1 {
                    warn!(
                        "scripts: a positioned sound waits for no bus: all {} script emitter buses are in use (counted)",
                        SCRIPT_BUSES.len()
                    );
                }
                None
            }
        }
    }
}

pub(super) fn play(
    mut sounds: ResMut<ScriptSounds>,
    game: Option<Res<GameAudio>>,
    time: Res<Time<Real>>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
) {
    let s = &mut *sounds;
    s.clock += time.delta_secs();
    while let Some((id, got)) = s.rx.lock().ok().and_then(|r| r.try_recv().ok()) {
        s.decoding.remove(&id);
        match got {
            Ok(sound) => s.keep(id, sound),
            Err(e) => {
                warn!("scripts: the sound {id} cannot play: {e}");
                s.failed.insert(id, e);
            }
        }
    }
    if s.pending.is_empty() && s.buses.iter().all(Option::is_none) {
        return;
    }
    let Some(game) = game else {
        s.pending.clear();
        return;
    };
    let Some(mut out) = game.lock() else { return };
    let Some(engine) = out.sfx.as_mut() else {
        s.pending.clear();
        s.buses = [None; SCRIPT_BUSES.len()];
        return;
    };
    if s.pending.iter().any(|o| o.position.is_some()) && engine.mixer.graph.submix_input(SCRIPT_BUSES[0]).is_none() {
        let n = engine.script_buses();
        info!("scripts: {n} script emitter buses (copies of Board_Center_BUS) for positioned sounds");
        s.buses = [None; SCRIPT_BUSES.len()];
    }
    let mut waiting = VecDeque::new();
    while let Some(o) = s.pending.pop_front() {
        if o.id.namespace() == BASE_NAMESPACE {
            let disc = s.content.as_ref().and_then(|c| c.disc_sounds());
            let Some(d) = disc.as_ref().and_then(|d| d.get(&o.id)) else { continue };
            let Some(key) = engine.aems.resolve(d.csis, &d.name) else {
                info!("scripts: [{}] {}: not in the level's loaded sound banks", o.package, o.id);
                continue;
            };
            let origin = match o.position {
                Some(p) => match s.take_bus(|b| engine.bus_senders(b), p) {
                    Some(k) => Some(SCRIPT_ORIGIN + k as u32),
                    None => continue,
                },
                None => None,
            };
            if d.function {
                let h = engine.aems.call_from(key, &[], origin);
                engine.aems.stop(h);
            } else {
                match origin {
                    Some(origin) => engine.fire_from(key, &[], origin),
                    None => engine.fire(key, &[]),
                }
            }
            s.logged(&o);
            continue;
        }
        if s.failed.contains_key(&o.id) {
            continue;
        }
        match s.held.get(&o.id).cloned() {
            Some(sound) => {
                let Some(draw) = sound.draw() else { continue };
                let Some((patch, v)) = engine.mixer.voice_table().values().find(|(_, v)| v == VOICE).cloned() else {
                    warn!("scripts: no {VOICE} in the level's voice table: {} not played", o.id);
                    continue;
                };
                if engine.mixer.released_voices() >= MAX_VOICES {
                    s.refused_voices += 1;
                    if s.refused_voices == 1 {
                        warn!(
                            "scripts: a package sound is refused: {MAX_VOICES} package voices play at once (counted)"
                        );
                    }
                    continue;
                }
                let bus = match o.position {
                    Some(p) => match s.take_bus(|b| engine.bus_senders(b), p) {
                        Some(k) => Some(k),
                        None => continue,
                    },
                    None => None,
                };
                let label = format!("script {}", o.id);
                let Some(h) = engine.mixer.start_package(&patch, &v, draw.sample, draw.pitch, sound.volume, label)
                else {
                    continue;
                };
                engine.mixer.release_when_ended(h);
                if let Some(k) = bus
                    && !engine.mixer.route_pcm(h, SCRIPT_BUSES[k])
                {
                    warn!("scripts: {} plays without its position: no {} bus", o.id, SCRIPT_BUSES[k]);
                }
                s.logged(&o);
            }
            None => {
                s.decode(&o.id);
                if !s.failed.contains_key(&o.id) {
                    waiting.push_back(o);
                }
            }
        }
    }
    s.pending = waiting;
    let now = s.clock;
    for (k, b) in s.buses.iter_mut().enumerate() {
        if b.is_some_and(|p| now - p.since >= BUS_HOLD) && engine.bus_senders(SCRIPT_BUSES[k]) == 0 {
            *b = None;
        }
    }
    let emitters: Vec<Emitter> = s
        .buses
        .iter()
        .enumerate()
        .filter_map(|(k, b)| {
            b.map(|p| Emitter {
                bus: SCRIPT_BUSES[k],
                settings: SCRIPT_EMITTER,
                base: p.position,
                rotation: [0.0, 0.0, 0.0, 1.0],
                velocity: [0.0; 3],
            })
        })
        .collect();
    if !emitters.is_empty() {
        let camera = cameras.iter().next().map(|t| (t.translation().to_array(), (*t.forward()).to_array()));
        engine.sound3d_frame_scripts(camera, &emitters, time.delta_secs());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn positioned_sounds_take_the_eight_emitter_buses() {
        let mut s = ScriptSounds::default();
        let taken: Vec<_> = (0..8).map(|i| s.take_bus(|_| 0, [i as f32, 0.0, 0.0])).collect();
        assert_eq!(taken, (0..8).map(Some).collect::<Vec<_>>());
        assert_eq!(s.take_bus(|_| 0, [9.0, 0.0, 0.0]), None);
        assert_eq!(s.refused_3d, 1);
        s.clock += BUS_HOLD;
        let senders = |b: &str| usize::from(b == SCRIPT_BUSES[0]);
        assert_eq!(s.take_bus(senders, [1.0, 2.0, 3.0]), Some(1));
        assert_eq!(s.buses[1].map(|p| p.position), Some([1.0, 2.0, 3.0]));
        let busy = |b: &str| usize::from(b == SCRIPT_BUSES[2]);
        assert_eq!(s.take_bus(busy, [0.0; 3]), Some(0));
        assert_eq!(s.take_bus(busy, [0.0; 3]), Some(3), "bus 2 is held by its voice");
    }
}
