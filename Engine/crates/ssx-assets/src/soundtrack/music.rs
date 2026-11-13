use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use ssx_formats::eaac::{Block, SoundHeader, read_v0_blocks};
use ssx_formats::ealayer3_decode::SectionDecoder;
use ssx_formats::mpf::{MpfFile, MusFile};

use crate::pathfinder::{PITCH_ONE, Pathfinder, SAMPLE_RATE, SegmentLengths, TICK_MS};

pub const SSXDD_EVENTS: [(&str, u32); 43] = [
    ("DD_ID_01", 0x02CD_5E1F),
    ("DD_ID_02", 0x02C4_103C),
    ("DD_ID_03", 0x02C3_3FA1),
    ("DD_ID_04", 0x02A7_AA7A),
    ("DD_ID_05", 0x02A0_9A35),
    ("DD_ID_06", 0x02A9_CC24),
    ("DD_ID_07", 0x02AE_FBAB),
    ("DD_ID_08", 0x0211_E68A),
    ("DD_ID_09", 0x0216_D643),
    ("DD_ID_10", 0x02D1_5F48),
    ("Music_Intro", 0x0237_B78F),
    ("Music_Start", 0x024B_2769),
    ("Music_Hover_Loop", 0x023D_E7C1),
    ("Music_Event_A", 0x0256_F68F),
    ("Music_Event_B", 0x025F_A6D2),
    ("Music_Event_C", 0x0258_978D),
    ("Music_WTF_A", 0x0262_E08E),
    ("Music_WTF_B", 0x026B_B1CF),
    ("Music_WTF_REV", 0x02BB_508A),
    ("Music_Return", 0x02E8_E2AD),
    ("Danger_Zone", 0x02E0_C8FB),
    ("Tricky_A", 0x0266_AFBF),
    ("Tricky_B", 0x026F_FE9C),
    ("Outro_Lose", 0x02FD_67D4),
    ("Outro_Win", 0x02D9_0DEB),
    ("Outro_Death_fx", 0x02F4_E459),
    ("Outro_Death", 0x0209_34A7),
    ("Music_Restart", 0x021D_0E0F),
    ("Music_STOP", 0x02D2_48D8),
    ("Intensity_110", 0x028B_EF77),
    ("Intensity_70", 0x0299_A0E9),
    ("Intensity_90", 0x021A_8D05),
    ("Intensity_50", 0x02AF_C39D),
    ("Intensity_30", 0x02F5_63C1),
    ("Intensity_10", 0x02C3_0635),
    ("Music_Vol_Full", 0x028A_294A),
    ("Music_Vol_Zero", 0x022B_0C23),
    ("Music_Fade_In_A", 0x028F_B7C4),
    ("Music_Fade_In_B", 0x0286_E9D7),
    ("Music_Fade_In_C", 0x0281_D996),
    ("Music_Fade_Out_A", 0x02B2_76CF),
    ("Music_Fade_Out_B", 0x02BB_26CC),
    ("Music_Fade_Out_C", 0x02BC_1751),
];

pub const DEADLY_BANKS: [(&str, u32); 12] = [
    ("Invalid", 0),
    ("AlsoInvalid", 0),
    ("Rockies", 0x1200_0004),
    ("Patagonia", 0x1200_0008),
    ("Siberia", 0x1200_0040),
    ("Alaska", 0x1200_0002),
    ("Antarctica", 0x1200_0010),
    ("Himalayas", 0x1200_0080),
    ("Alps", 0x1200_0200),
    ("Africa", 0x1200_0020),
    ("NewZealand", 0x1200_0100),
    ("Lhotse", 0x1200_0400),
];

pub fn bank_for_chapter(chapter: u32) -> Option<usize> {
    let mask = DEADLY_BANKS.get(chapter as usize)?.1 & 0xFF_FFFF;
    (mask != 0).then(|| mask.trailing_zeros() as usize)
}

pub fn ssxdd_event(name: &str) -> Option<u32> {
    let name = name.strip_prefix("PATH_EVENT_SSXDD_GLOBAL_").unwrap_or(name);
    SSXDD_EVENTS.iter().find(|(n, _)| *n == name).map(|&(_, v)| v)
}

pub fn find_file(big_root: &Path, rel: &str) -> Option<PathBuf> {
    crate::data_roots::find(big_root, rel)
}

pub fn find_files(big_root: &Path, rel: &str, ext: &str) -> Vec<PathBuf> {
    crate::data_roots::files_in(big_root, rel, ext)
}

pub enum Bytes {
    Map(memmap2::Mmap),
    Vec(Vec<u8>),
}

impl std::ops::Deref for Bytes {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Bytes::Map(m) => m,
            Bytes::Vec(v) => v,
        }
    }
}

impl Bytes {
    pub fn open(path: &Path) -> Result<Self> {
        let f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
        match unsafe { memmap2::Mmap::map(&f) } {
            Ok(m) => Ok(Bytes::Map(m)),
            Err(_) => Ok(Bytes::Vec(std::fs::read(path)?)),
        }
    }
}

pub struct TrackAudio {
    pub data: Arc<Bytes>,
    pub index: MusFile,
    pub ram: bool,
    pub first_sample: usize,
}

impl TrackAudio {
    fn sound(&self, e: usize) -> Result<(SoundHeader, usize)> {
        let entry = self.index.entries.get(e).with_context(|| format!("no .mus entry {e}"))?;
        let h = SoundHeader::parse(&self.data[entry.snr_offset as usize..])?;
        let body = if self.ram { entry.snr_offset as usize + h.size } else { entry.sns_offset as usize };
        Ok((h, body))
    }
}

struct SoundCursor {
    track: usize,
    blocks: Vec<(usize, usize, u32)>,
    next: usize,
    channels: usize,
    dec: Option<(usize, SectionDecoder)>,
}

impl SoundCursor {
    fn open(audio: &TrackAudio, track: usize, entry: usize) -> Result<Self> {
        let (h, body) = audio.sound(entry)?;
        let blocks = read_v0_blocks(&h, &audio.data, body)?;
        let list = blocks
            .sections
            .iter()
            .enumerate()
            .flat_map(|(s, sec)| sec.iter().map(move |b| (b.offset, s, b.samples)))
            .collect();
        Ok(Self { track, blocks: list, next: 0, channels: h.channels as usize, dec: None })
    }

    fn decode_block(&mut self, audio: &TrackAudio, out: &mut Vec<f32>, tmp: &mut Vec<f32>) -> Result<bool> {
        let Some(&(offset, section, samples)) = self.blocks.get(self.next) else { return Ok(false) };
        self.next += 1;
        if self.dec.as_ref().is_none_or(|(s, _)| *s != section) {
            self.dec = Some((section, SectionDecoder::new(self.channels as u8)?));
        }
        let block = Block::read(&audio.data, offset)?;
        tmp.clear();
        let n = self.dec.as_mut().unwrap().1.decode_payload(block.payload, tmp)?;
        let n = n.min(samples as usize);
        for f in 0..n {
            if self.channels == 1 {
                out.extend_from_slice(&[tmp[f], tmp[f]]);
            } else {
                out.extend_from_slice(&tmp[f * self.channels..f * self.channels + 2]);
            }
        }
        Ok(true)
    }
}

#[derive(Default)]
struct Voice {
    item: Option<u64>,
    sounds: VecDeque<(usize, usize)>,
    cur: Option<SoundCursor>,
    buf: Vec<f32>,
    base: usize,
    pos: f64,
    reported: f64,
    gain: f32,
}

impl Voice {
    fn open(&mut self, id: u64, sounds: VecDeque<(usize, usize)>, carry: f64) {
        self.item = Some(id);
        self.sounds = sounds;
        self.cur = None;
        self.buf.clear();
        self.base = 0;
        self.pos = carry;
        self.reported = 0.0;
    }

    fn frame(&mut self, f: usize, tracks: &[Option<TrackAudio>], tmp: &mut Vec<f32>) -> [f32; 2] {
        while self.base + self.buf.len() / 2 <= f {
            if !self.decode_more(tracks, tmp) {
                return [0.0; 2];
            }
        }
        let k = (f.max(self.base) - self.base) * 2;
        [self.buf[k], self.buf[k + 1]]
    }

    fn decode_more(&mut self, tracks: &[Option<TrackAudio>], tmp: &mut Vec<f32>) -> bool {
        let keep_from = (self.pos.floor() as usize).saturating_sub(1).max(self.base);
        if keep_from > self.base + 4096 {
            let drop = (keep_from - self.base).min(self.buf.len() / 2);
            self.buf.drain(..drop * 2);
            self.base += drop;
        }
        loop {
            if self.cur.is_none() {
                let Some((t, e)) = self.sounds.pop_front() else { return false };
                let Some(audio) = tracks.get(t).and_then(Option::as_ref) else { return false };
                match SoundCursor::open(audio, t, e) {
                    Ok(c) => self.cur = Some(c),
                    Err(err) => {
                        ssx_core::eprintln!("music: track {t} entry {e}: {err:#}");
                        continue;
                    }
                }
            }
            let cur = self.cur.as_mut().unwrap();
            let audio = tracks[cur.track].as_ref().unwrap();
            match cur.decode_block(audio, &mut self.buf, tmp) {
                Ok(true) => return true,
                Ok(false) => self.cur = None,
                Err(err) => {
                    ssx_core::eprintln!("music: decoding track {}: {err:#}", cur.track);
                    self.cur = None;
                }
            }
        }
    }
}

const TICK_FRAMES: usize = (SAMPLE_RATE / 1000 * TICK_MS) as usize;

pub struct PathfinderMusic {
    pub rt: Pathfinder,
    tracks: Vec<Option<TrackAudio>>,
    voices: Vec<Voice>,
    tick_left: usize,
    tmp: Vec<f32>,
    overrides: Vec<Option<crate::track_override::TrackOverride>>,
    cursors: Vec<crate::track_override::OverrideCursor>,
    score: Option<crate::score::ScorePlayer>,
}

impl PathfinderMusic {
    pub fn load(big_root: &Path, name: &str, banks: &[usize]) -> Result<Self> {
        let dir = format!("data/xenon/audio/pathfinder/{name}");
        let mpf_path =
            find_file(big_root, &format!("{dir}/{name}.mpf")).with_context(|| format!("{dir}/{name}.mpf"))?;
        let mpf = Arc::new(MpfFile::parse(&std::fs::read(&mpf_path)?).context("parsing the .mpf")?);
        let mut by_word = Vec::new();
        for p in find_files(big_root, &dir, "mus") {
            let data = Arc::new(Bytes::open(&p)?);
            if data.len() >= 4 {
                by_word.push((u32::from_be_bytes(data[..4].try_into().unwrap()), data));
            }
        }
        let mut tracks = Vec::new();
        let mut lengths = SegmentLengths { records: vec![0; mpf.samples.len()], entries: Vec::new() };
        for tr in &mpf.tracks {
            let ram = tr.num_subbanks > 0;
            let word = if ram { tr.subbanks[0][0] } else { tr.checksum };
            let audio = by_word.iter().find(|(w, _)| *w == word).map(|(_, d)| d.clone());
            let Some(data) = audio else {
                tracks.push(None);
                lengths.entries.push(Vec::new());
                continue;
            };
            let index = MusFile::parse(&data)?;
            let audio = TrackAudio { data, index, ram, first_sample: tr.first_sample as usize };
            let mut frames = Vec::with_capacity(audio.index.entries.len());
            for e in 0..audio.index.entries.len() {
                frames.push(audio.sound(e).map_or(0, |(h, _)| h.num_samples));
            }
            if ram {
                lengths.entries.push(frames);
            } else {
                for (k, &f) in frames.iter().enumerate() {
                    if let Some(r) = lengths.records.get_mut(audio.first_sample + k) {
                        *r = f;
                    }
                }
                lengths.entries.push(Vec::new());
            }
            tracks.push(Some(audio));
        }
        if tracks.iter().all(Option::is_none) {
            bail!("no .mus of {name} found under {}", big_root.display());
        }
        let mut rt = Pathfinder::new(mpf.clone(), lengths);
        if name == "ssxdd" {
            rt.names = SSXDD_EVENTS.iter().map(|&(n, v)| (v & 0xFF_FFFF, n.to_string())).collect();
        }
        for (t, tr) in mpf.tracks.iter().enumerate() {
            if tracks[t].is_some() && (tr.num_subbanks == 0 || banks.contains(&t)) {
                rt.add_player(t, tr.num_subbanks > 0);
            }
        }
        let voices = (0..crate::pathfinder::MAX_PLAYERS).map(|_| Voice::default()).collect();
        Ok(Self {
            rt,
            tracks,
            voices,
            tick_left: 0,
            tmp: Vec::new(),
            overrides: Vec::new(),
            cursors: Vec::new(),
            score: None,
        })
    }

    pub fn track_heads(&self, track: usize) -> Result<Vec<(usize, u32, u32, u16)>> {
        let audio = self.tracks.get(track).and_then(Option::as_ref).context("no such track with audio")?;
        let mut out = Vec::new();
        for e in 0..audio.index.entries.len() {
            let (h, _) = audio.sound(e)?;
            let record = if audio.ram { e } else { audio.first_sample + e };
            out.push((record, h.num_samples, h.sample_rate, u16::from(h.channels)));
        }
        Ok(out)
    }

    pub fn decode_track_sound(&self, track: usize, k: usize) -> Result<Vec<f32>> {
        let audio = self.tracks.get(track).and_then(Option::as_ref).context("no such track with audio")?;
        let (h, body) = audio.sound(k)?;
        let blocks = read_v0_blocks(&h, &audio.data, body)?;
        Ok(ssx_formats::ealayer3_decode::decode_sound(&h, &blocks)?.samples)
    }

    pub fn set_track_override(&mut self, track: usize, o: crate::track_override::TrackOverride) {
        if track >= crate::pathfinder::MAX_PLAYERS {
            return;
        }
        if self.overrides.is_empty() {
            self.overrides = vec![None; crate::pathfinder::MAX_PLAYERS];
            self.cursors = vec![Default::default(); crate::pathfinder::MAX_PLAYERS];
        }
        self.rt.logf(format!("p{track}: {}", o.label));
        self.overrides[track] = Some(o);
    }

    pub fn clear_packages(&mut self) {
        if self.overrides.iter().any(Option::is_some) || self.score.is_some() {
            self.rt.logf("the package's music removed: the project's own again".into());
        }
        self.overrides.clear();
        self.cursors.clear();
        self.score = None;
        self.rt.score = None;
    }

    pub fn set_score(&mut self, score: crate::score::ScorePlayer, label: &str) {
        self.rt.logf(format!("score: {label} in place of the project's"));
        self.rt.score = Some(crate::score::ScoreInbox::default());
        self.score = Some(score);
    }

    pub fn bank_for_part(mpf: &MpfFile, part: u8) -> Option<usize> {
        mpf.nodes
            .iter()
            .find(|n| n.sample == 0 && n.part == part && mpf.tracks[n.track as usize].num_subbanks > 0)
            .map(|n| n.track as usize)
    }

    pub fn render(&mut self, out: &mut [f32]) {
        let n = out.len() / 2;
        out.fill(0.0);
        let mut done = 0;
        while done < n {
            if self.tick_left == 0 {
                self.rt.tick();
                if let (Some(score), Some(inbox)) = (self.score.as_mut(), self.rt.score.as_mut()) {
                    score.take(inbox);
                    let ms = self.rt.now;
                    for text in score.log.drain(..) {
                        self.rt.log.push(crate::pathfinder::LogLine { ms, text });
                    }
                }
                self.tick_left = TICK_FRAMES;
            }
            let chunk = (n - done).min(self.tick_left);
            if let Some(score) = self.score.as_mut() {
                score.render(&mut out[done * 2..(done + chunk) * 2]);
            } else {
                for i in 0..self.voices.len() {
                    if self.rt.players[i].is_some() {
                        self.render_voice(i, &mut out[done * 2..(done + chunk) * 2]);
                    }
                }
            }
            self.rt.frame += chunk as u64;
            self.tick_left -= chunk;
            done += chunk;
        }
    }
}

impl PathfinderMusic {
    fn item_sounds(&self, node: u16, record: usize, piece: u8) -> VecDeque<(usize, usize)> {
        let n = &self.rt.mpf.nodes[node as usize];
        let t = n.track as usize;
        let Some(audio) = self.tracks.get(t).and_then(Option::as_ref) else { return VecDeque::new() };
        if !audio.ram {
            return VecDeque::from([(t, record - audio.first_sample)]);
        }
        let first = (self.rt.mpf.samples[record].location & 0xFFFF) as usize;
        if piece > 0 {
            return VecDeque::from([(t, first + usize::from(piece) - 1)]);
        }
        let k = (usize::from(n.beats_per_bar) * usize::from(n.bars)).max(1);
        (first..first + k).map(|e| (t, e)).collect()
    }

    fn render_voice(&mut self, i: usize, out: &mut [f32]) {
        let frames = out.len() / 2;
        let Some(p) = self.rt.players[i].as_ref() else { return };
        if p.paused {
            return;
        }
        let target = p.gain();
        let ratio = f64::from(p.pitch) / f64::from(PITCH_ONE);
        let idle = self.voices[i].item.is_none() && self.voices[i].pos == 0.0;
        let g0 = if idle { target } else { self.voices[i].gain };
        self.voices[i].gain = target;
        for f in 0..frames {
            let Some(front) = self.rt.players[i].as_ref().and_then(|p| p.stream.items.front()) else {
                self.voices[i].item = None;
                self.voices[i].pos = 0.0;
                if let Some(c) = self.cursors.get_mut(i) {
                    c.started = false;
                }
                break;
            };
            if front.start_frame.is_some_and(|s| self.rt.frame + (f as u64) < s) {
                continue;
            }
            let (id, total) = (front.id, f64::from(front.frames));
            if self.voices[i].item != Some(id) {
                let carry = if self.voices[i].item.is_none() { self.voices[i].pos } else { 0.0 };
                let sounds = self.item_sounds(front.node, front.record, front.piece);
                let at = self.rt.frame + f as u64;
                let what = if front.piece > 0 { format!(" beat {}", front.piece) } else { String::new() };
                let text = format!("p{i} play n{}{what} at frame {at}", front.node);
                let ms = (at * 1000 / u64::from(SAMPLE_RATE)) as u32;
                self.rt.log.push(crate::pathfinder::LogLine { ms, text });
                self.voices[i].open(id, sounds, carry);
                if let Some(o) = self.overrides.get(i).and_then(Option::as_ref) {
                    let section = self.rt.mpf.nodes.get(usize::from(front.node)).map_or(0, |n| n.section);
                    if let Some(text) = o.enter(&mut self.cursors[i], section) {
                        self.rt.log.push(crate::pathfinder::LogLine { ms, text });
                    }
                }
            }
            // TODO(guess): the Pathfinder voices run on this engine's 44.1 kHz 10 ms tick (gains ramp over the tick, float position) instead of the game's Send / Resample kernels (`sfx_dsp::ramp_gain` / `Resampler`, 256-sample 48 kHz blocks); settle: move the stream voices onto the 48 kHz block graph (`sfx_bus`).
            let g = g0 + (target - g0) * (f + 1) as f32 / frames as f32;
            let v = &mut self.voices[i];
            if let Some(o) = self.overrides.get(i).and_then(Option::as_ref) {
                let x = o.frame(&mut self.cursors[i], ratio);
                out[2 * f] += x[0] * g;
                out[2 * f + 1] += x[1] * g;
            } else {
                let k = v.pos.floor();
                let frac = (v.pos - k) as f32;
                let a = v.frame(k as usize, &self.tracks, &mut self.tmp);
                let b = if frac > 0.0 && k + 1.0 < total {
                    v.frame(k as usize + 1, &self.tracks, &mut self.tmp)
                } else {
                    a
                };
                out[2 * f] += (a[0] + (b[0] - a[0]) * frac) * g;
                out[2 * f + 1] += (a[1] + (b[1] - a[1]) * frac) * g;
            }
            v.pos += ratio;
            if v.pos >= total {
                let done = total - v.reported;
                v.pos -= total;
                v.item = None;
                v.reported = 0.0;
                self.rt.consume(i, done);
            }
        }
        let v = &mut self.voices[i];
        if v.item.is_some() {
            let d = v.pos - v.reported;
            v.reported = v.pos;
            self.rt.consume(i, d);
        }
    }
}
