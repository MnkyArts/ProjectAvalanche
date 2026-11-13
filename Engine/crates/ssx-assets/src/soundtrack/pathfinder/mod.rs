mod sync;
mod walk;

use std::collections::VecDeque;
use std::sync::Arc;

use ssx_formats::mpf::{self, MpfFile};

pub use sync::*;

pub const SAMPLE_RATE: u32 = 44_100;
pub const TICK_MS: u32 = 10;
pub const STREAM_TICKS: u32 = 5;
pub const STREAM_PREFETCH_MS: u32 = 400;
pub const BANK_PREFETCH_MS: u32 = 10;
pub const BANK_EARLY_MS: u32 = 50;
pub const MAX_PLAYERS: usize = 24;
pub const MAX_EVENTS: usize = 16;
pub const EVENT_ARENA: usize = 0x1000;
pub const DEFAULT_MASTER_VOLUME: u8 = 100;
pub const PITCH_ONE: u16 = 0x1000;

#[derive(Clone, Debug)]
pub struct Queued {
    pub id: u64,
    pub node: u16,
    pub record: usize,
    pub piece: u8,
    pub frames: u32,
    pub duration_ms: u32,
    pub start_frame: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct Stream {
    pub items: VecDeque<Queued>,
    pub head_frames: f64,
}

impl Stream {
    pub fn remaining_ms(&self, pitch: u16) -> u32 {
        let frames: f64 = self.items.iter().map(|q| f64::from(q.frames)).sum::<f64>() - self.head_frames;
        if pitch == 0 {
            return u32::MAX;
        }
        let out = frames.max(0.0) * f64::from(PITCH_ONE) / f64::from(pitch);
        (out * 1000.0 / f64::from(SAMPLE_RATE)) as u32
    }

    pub fn playing_node(&self) -> Option<u16> {
        self.items.front().map(|q| q.node)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Fade {
    pub start_ms: u32,
    pub duration_ms: u32,
    pub from: i32,
    pub to: i32,
    pub curve: i32,
}

impl Fade {
    pub fn value(&self, now: u32) -> (Option<i32>, bool) {
        let dur = self.duration_ms & 0xFFFF;
        let t = if dur == 0 { 1.0 } else { now.wrapping_sub(self.start_ms) as f32 / dur as f32 };
        if t >= 1.0 {
            return (Some(self.to), true);
        }
        let span = (self.to - self.from) as f32;
        let t = if span >= 0.0 { 1.0 - t } else { t };
        let f = match self.curve & 0x7F {
            1 => 1.0 - t,
            2 => -(t * t - 1.0),
            3 => (1.0 / t) * 0.04,
            _ => -1.0,
        };
        let base = self.from.min(self.to) as f32;
        if (0.0..=1.0).contains(&f) { (Some((f * span.abs() + base) as i32), false) } else { (None, false) }
    }
}

#[derive(Clone, Debug)]
pub struct Player {
    pub track: u8,
    pub bank: bool,
    pub active: bool,
    pub level: u8,
    pub master_volume: u8,
    pub volume: u8,
    pub current: i32,
    pub loop_node: i32,
    pub loop_left: u8,
    pub beat: i8,
    pub paused: bool,
    pub flag5: bool,
    pub flags_1_3: u8,
    pub section: Option<usize>,
    pub prefetch_ms: u32,
    pub start_at: u32,
    pub sync_frame: Option<u64>,
    pub pause_at: u32,
    pub resume_at: u32,
    pub volume_fade: Option<Fade>,
    pub pitch_fade: Option<Fade>,
    pub stream_volume: i32,
    pub pitch: u16,
    pub prop8: u16,
    pub prop9: u16,
    pub stream: Stream,
}

impl Player {
    fn new(track: u8, bank: bool) -> Self {
        Self {
            track,
            bank,
            active: true,
            level: 0,
            master_volume: DEFAULT_MASTER_VOLUME,
            volume: 0x7F,
            current: -1,
            loop_node: -1,
            loop_left: 0,
            beat: -1,
            paused: false,
            flag5: false,
            flags_1_3: 0,
            section: None,
            prefetch_ms: if bank { BANK_PREFETCH_MS } else { STREAM_PREFETCH_MS },
            start_at: 0,
            sync_frame: None,
            pause_at: 0,
            resume_at: 0,
            volume_fade: None,
            pitch_fade: None,
            stream_volume: 0x7F,
            pitch: PITCH_ONE,
            prop8: 0,
            prop9: 0,
            stream: Stream::default(),
        }
    }

    pub fn is_playing(&self) -> bool {
        self.current >= 0 && self.section.is_some()
    }

    pub fn gain(&self) -> f32 {
        if self.paused { 0.0 } else { self.stream_volume.clamp(0, 127) as f32 / 127.0 }
    }

    pub(crate) fn set_volume(&mut self, v: u8) {
        self.volume = v;
        self.stream_volume = i32::from(self.master_volume) * i32::from(v as i8) / 100;
        self.volume_fade = None;
    }
}

#[derive(Clone, Debug, Default)]
pub struct SegmentLengths {
    pub records: Vec<u32>,
    pub entries: Vec<Vec<u32>>,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub ms: u32,
    pub text: String,
}

pub struct Pathfinder {
    pub mpf: Arc<MpfFile>,
    pub lengths: SegmentLengths,
    pub now: u32,
    ticks: u64,
    pub frame: u64,
    pub players: Vec<Option<Player>>,
    pub(crate) events: Vec<crate::pathfinder_events::EventInstance>,
    pub variables: Vec<i32>,
    pub master: Option<usize>,
    rng: [u32; 6],
    next_id: u64,
    pub names: Vec<(u32, String)>,
    pub log: Vec<LogLine>,
    pub score: Option<crate::score::ScoreInbox>,
}

const RNG_STATE: [u32; 6] = [0xF22D_0E56, 0x8831_26E9, 0xC624_DD2F, 0x0702_C49C, 0x9E35_3F7D, 0x6FDF_3B64];

impl Pathfinder {
    pub fn new(mpf: Arc<MpfFile>, lengths: SegmentLengths) -> Self {
        let variables = vec![0; mpf.variables.len()];
        Self {
            mpf,
            lengths,
            now: 0,
            ticks: 0,
            frame: 0,
            players: vec![None; MAX_PLAYERS],
            events: Vec::new(),
            variables,
            master: None,
            rng: RNG_STATE,
            next_id: 0,
            names: Vec::new(),
            log: Vec::new(),
            score: None,
        }
    }

    pub fn add_player(&mut self, track: usize, bank: bool) {
        if track < MAX_PLAYERS {
            self.players[track] = Some(Player::new(track as u8, bank));
        }
    }

    pub(crate) fn logf(&mut self, text: String) {
        self.log.push(LogLine { ms: self.now, text });
    }

    pub fn event_name(&self, id: u32) -> String {
        let id = id & 0xFF_FFFF;
        self.names.iter().find(|(i, _)| *i & 0xFF_FFFF == id).map_or_else(
            || match self.mpf.event_by_id(id) {
                Some(i) => format!("ev{i}"),
                None => format!("event {id:#08x}"),
            },
            |(_, n)| n.clone(),
        )
    }

    pub fn post(&mut self, value: u32) -> bool {
        if value & 0x0F00_0000 & (0x0100_0000 << self.mpf.project) == 0 {
            return false;
        }
        let id = value & 0xFF_FFFF;
        let Some(e) = self.mpf.event_by_id(id) else { return false };
        let name = self.event_name(id);
        if let Some(inbox) = self.score.as_mut() {
            inbox.posts.push((value, name.clone()));
            self.logf(format!("post {name} → the package's score"));
            return true;
        }
        self.logf(format!("post {name}"));
        self.instantiate(e)
    }

    // PORT(choice): the disc's latency is not modelled — a stream never reports 2 (its segments are in memory) and the
    // front-end module's map load (0x830D0BC8: states 1 → 2 → 3) completes at its next update; enough unless the port
    // streams from a slow medium; the console streams from its disc.
    pub fn stream_state(&self, i: usize) -> u8 {
        if let Some(inbox) = &self.score {
            return if self.players.get(i).and_then(Option::as_ref).is_some() { inbox.state } else { 0 };
        }
        let Some(p) = self.players.get(i).and_then(Option::as_ref) else { return 0 };
        if p.paused {
            return 3;
        }
        if p.stream.items.is_empty() {
            return 1;
        }
        let mut st = 5;
        if p.stream.items.len() == 1 && p.stream.remaining_ms(p.pitch) <= p.prefetch_ms {
            st = 4;
        }
        if p.volume_fade.is_some() {
            st = 6;
        }
        st
    }

    pub fn set_control_level(&mut self, level: u32) {
        if level < 0x80
            && let Some(inbox) = self.score.as_mut()
        {
            inbox.control = Some(level);
        }
        if level < 0x80 {
            for p in self.players.iter_mut().flatten() {
                p.level = level as u8;
            }
            self.logf(format!("control level {level}"));
        }
    }

    pub fn set_player_level(&mut self, mask: u32, level: u32) {
        if level < 0x80 {
            for (i, p) in self.players.iter_mut().enumerate() {
                if let Some(p) = p
                    && mask & 0xFF_FFFF & (1 << i) != 0
                {
                    p.level = level as u8;
                }
            }
            self.logf(format!("control level {level} (players {:#x})", mask & 0xFF_FFFF));
        }
    }

    pub fn cancel_events(&mut self) {
        self.events.clear();
    }

    pub fn set_master_volume(&mut self, v: i32) {
        if let Some(inbox) = self.score.as_mut() {
            inbox.volume = Some(v);
        }
        let v = v.clamp(0, 100) as u8;
        for p in self.players.iter_mut().flatten() {
            p.master_volume = v;
            if p.volume_fade.is_none() {
                p.stream_volume = i32::from(v) * i32::from(p.volume as i8) / 100;
            }
        }
    }

    pub fn stop_all(&mut self) {
        for p in self.players.iter_mut().flatten() {
            p.stream.items.clear();
            p.stream.head_frames = 0.0;
            p.current = -1;
            p.section = None;
            p.beat = -1;
        }
        self.events.clear();
        if let Some(inbox) = self.score.as_mut() {
            inbox.stop = true;
        }
        self.logf("stop".into());
    }

    pub fn set_paused(&mut self, paused: bool) {
        if let Some(inbox) = self.score.as_mut() {
            inbox.paused = Some(paused);
        }
        for p in self.players.iter_mut().flatten() {
            p.paused = paused;
        }
    }

    pub(crate) fn random(&mut self) -> u32 {
        let s = &mut self.rng;
        let (s4, c) = s[4].overflowing_add(s[5]);
        let add = |a: u32, b: u32, c: bool| -> (u32, bool) {
            let r = a.wrapping_add(b).wrapping_add(u32::from(c));
            (r, r < a)
        };
        let (s3, c) = add(s[3], s4, c);
        let (s2, c) = add(s[2], s3, c);
        let (s1, c) = add(s[1], s2, c);
        let (s0, _) = add(s[0], s1, c);
        *s = [s0, s1, s2, s3, s4, s[5].wrapping_add(1)];
        for k in (0..5).rev() {
            if s[k + 1] != 0 {
                break;
            }
            s[k] = s[k].wrapping_add(1);
        }
        s[0]
    }

    pub(crate) fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_map() -> Arc<MpfFile> {
        let mut d = vec![0u8; 0x48];
        d[..4].copy_from_slice(b"PFDx");
        d[4] = 5;
        d[5] = 2;
        for (k, off) in [0x48u32, 0x48, 0x48, 0x48, 0x48, 0x48, 0x48, 0x48, 0x48, 0x48].iter().enumerate() {
            d[0x14 + 4 * k..0x18 + 4 * k].copy_from_slice(&off.to_be_bytes());
        }
        Arc::new(MpfFile::parse(&d).unwrap())
    }

    #[test]
    fn rng_matches_the_exe_sequence() {
        let mut pf = Pathfinder::new(empty_map(), SegmentLengths::default());
        let got: Vec<u32> = (0..5).map(|_| pf.random()).collect();
        assert_eq!(got, [0x559A_51ED, 0x274E_A7F5, 0xE827_F7E0, 0x76B6_5632, 0x7F1B_3FEB]);
    }

    #[test]
    fn fades_follow_the_curves() {
        let up = Fade { start_ms: 0, duration_ms: 4000, from: 0, to: 127, curve: 1 };
        assert_eq!(up.value(0), (Some(0), false));
        assert_eq!(up.value(2000), (Some(63), false));
        assert_eq!(up.value(4000), (Some(127), true));
        let down = Fade { start_ms: 0, duration_ms: 2000, from: 4096, to: 0, curve: 1 };
        assert_eq!(down.value(500), (Some(3072), false));
        let ease = Fade { curve: 2, ..up };
        assert_eq!(ease.value(2000).0, Some(95));
        assert_eq!(Fade { curve: 0x7F, ..up }.value(1000), (None, false));
    }

    #[test]
    fn a_stopped_stream_reports_no_node_and_zeros_scaled_by_its_pitch() {
        let mut pf = Pathfinder::new(empty_map(), SegmentLengths::default());
        assert!(pf.first_status().is_none(), "no player");
        pf.add_player(1, false);
        let s = pf.first_status().expect("player 1");
        assert_eq!((s.node, s.duration, s.beat, s.to_beat, s.beat_number), (None, 0, 0, 0, 0));
        pf.players[1].as_mut().expect("player 1").pitch = 0x2000;
        let s = pf.status(1);
        assert_eq!((s.node, s.bar, s.remaining, s.to_bar), (None, 0, 0, 0), "zeros scaled stay zeros");
        pf.players[1].as_mut().expect("player 1").pitch = 0;
        let s = pf.status(1);
        let m = 0x7FFF_FFFF;
        assert_eq!([s.duration, s.bar, s.beat, s.remaining, s.to_bar, s.to_beat], [m; 6], "a pitch of 0");
        assert_eq!((s.node, s.beat_number), (None, 0));
    }

    #[test]
    fn sync_starts_on_the_next_master_beat() {
        let node = mpf::Node {
            offset: 0,
            sample: 1,
            track: 9,
            part: 8,
            loop_count: 0,
            remap: 0,
            level_mode: 0,
            beats_per_bar: 4,
            bars: 1,
            section: 0,
            beat_unit: 4,
            sync_mode: 0,
            unk_w2: 0,
            word3: mpf::FLAG_BEAT_SLICED,
            branches: Vec::new(),
        };
        let master = StreamStatus { duration: 1714, remaining: 500, beat: 428, ..Default::default() };
        let (start, piece) = sync_start(&node, 1714, 3, &master);
        assert_eq!((start, piece), (71, 1));
        let (start, piece) = sync_start(&node, 1714, 1, &master);
        assert_eq!((start, piece), (71, 4));
    }
}
