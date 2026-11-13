use std::path::Path;
use std::sync::Arc;

mod dlc;
mod package;
mod posts;
pub mod selection;

use dlc::MAX_BANK_BYTES;
pub use dlc::{BankFiles, DlcMusic, music_sets};
pub use package::{FIRST_PACKAGE_SONG, PackageSongs, SongAudio, SongSwitches};
pub use posts::{RACE_EVENT_PERIOD_MS, RemixClock, remix};
pub use selection::{RaceEvent, Session, session};

use posts::{RacePosts, ms_frames};

use anyhow::{Context, Result, bail};
use ssx_formats::eaac::Sps;
use ssx_formats::ealayer3_decode::SectionDecoder;
use ssx_formats::reader::Reader;
use ssx_formats::sbr::{SampleBank, SoundRef};
use ssx_formats::speech::{evt::EventSystem, grf::Graffiti, rep::RepetitionPools};

use crate::music::{Bytes, find_file};

#[derive(Clone, Debug)]
pub struct Song {
    pub id: u32,
    pub unk: [u32; 6],
    pub info: Vec<(String, String)>,
}

impl Song {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.info.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    pub fn flag(&self, key: &str) -> bool {
        self.get(key) == Some("True")
    }
    pub fn label(&self) -> String {
        format!("{} - {}", self.get("Artist").unwrap_or("?"), self.get("Title").unwrap_or("?"))
    }

    pub fn level(&self) -> u32 {
        self.unk[2]
    }

    pub fn weight(&self) -> u32 {
        self.unk[4]
    }

    pub fn default_weight(&self) -> u32 {
        self.unk[3]
    }
}

pub fn select_levelled(songs: &[Song], candidates: &[u32]) -> Vec<u32> {
    let mut level = 1i32;
    let mut out = Vec::new();
    for &id in candidates {
        let Some(s) = songs.iter().find(|s| s.id == id) else { continue };
        if s.weight() == 0 || (s.level() as i32) < level {
            continue;
        }
        if s.level() as i32 > level {
            out.clear();
            level = s.level() as i32;
        }
        out.push(id);
        if s.level() == 3 {
            break;
        }
    }
    out
}

fn cstr(d: &[u8], o: usize) -> Result<String> {
    let s = d.get(o..).context("string offset past the end")?;
    let end = s.iter().position(|&b| b == 0).context("unterminated string")?;
    Ok(String::from_utf8_lossy(&s[..end]).into_owned())
}

pub fn parse_song_info(d: &[u8]) -> Result<Vec<Song>> {
    let mut r = Reader::be(d);
    r.expect(b"MBSI")?;
    let mut h = [0u32; 6];
    for x in &mut h {
        *x = r.u32()?;
    }
    let (count, first) = (h[4] as usize, h[5] as usize);
    const RECORD: usize = 0x24;
    first
        .checked_add(count.checked_mul(RECORD).context("MBSI count")?)
        .filter(|&e| e <= d.len())
        .context("MBSI records past the end")?;
    let mut songs = Vec::with_capacity(count);
    for i in 0..count {
        let o = first + i * RECORD;
        r.seek(o)?;
        let id = r.u32()?;
        let mut unk = [0u32; 6];
        for x in &mut unk {
            *x = r.u32()?;
        }
        let (pairs, table) = (r.u32()? as usize, r.u32()? as usize);
        if pairs > 64 {
            bail!("MBSI record {id:#x}: {pairs} string pairs");
        }
        let mut info = Vec::with_capacity(pairs);
        for p in 0..pairs {
            r.seek(table.checked_add(8 * p).context("MBSI string table")?)?;
            let (k, v) = (r.u32()? as usize, r.u32()? as usize);
            info.push((cstr(d, k)?, cstr(d, v)?));
        }
        songs.push(Song { id, unk, info });
    }
    Ok(songs)
}

pub fn parse_graffiti_ids(d: &[u8]) -> Result<Vec<u32>> {
    let mut r = Reader::be(d);
    r.expect(b"GRF0")?;
    r.seek(0x30)?;
    if r.u32()? != 0x18 {
        bail!("GRF0: unexpected layout");
    }
    let n = r.u32()? as usize;
    (0..n).map(|_| Ok(r.u32()?)).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RaceType {
    Menu,
    RaceIt,
    TrickIt,
    SurviveIt,
}

pub const TUTORIAL_DROPS: [u32; 2] = [0xC145_699A, 0xB0F3_51A2];

impl RaceType {
    pub fn from_event(ui_drop: u32, kind: i32) -> Option<Self> {
        let class = if TUTORIAL_DROPS.contains(&ui_drop) { 2 } else { kind };
        match class {
            1 | 5 => Some(RaceType::RaceIt),
            2 => Some(RaceType::TrickIt),
            3 | 4 => Some(RaceType::SurviveIt),
            _ => None,
        }
    }

    fn flag(self) -> Option<&'static str> {
        match self {
            RaceType::Menu => None,
            RaceType::RaceIt => Some("RaceIt"),
            RaceType::TrickIt => Some("TrickIt"),
            RaceType::SurviveIt => Some("SurviveIt"),
        }
    }
}

struct Bank {
    bank: SampleBank,
    data: Vec<u8>,
    sbs: Arc<Bytes>,
}

pub struct MusicBox {
    pub songs: Vec<Song>,
    pub selectable: Vec<u32>,
    pub selections: Vec<selection::SelectionData>,
    pub events: EventSystem,
    pub graffiti: Vec<Graffiti>,
    pub pools: RepetitionPools,
    banks: Vec<Bank>,
    pub package: PackageSongs,
    pub package_switches: std::collections::BTreeMap<ssx_content::ContentId, bool>,
    pub dlc_sets: Vec<String>,
}

impl MusicBox {
    pub fn load(big_root: &Path) -> Result<Self> {
        let rel = |f: &str| format!("data/xenon/audio/music/{f}");
        let read = |f: &str| -> Result<Vec<u8>> {
            let p = find_file(big_root, &rel(f)).with_context(|| format!("{} not found", rel(f)))?;
            Ok(std::fs::read(p)?)
        };
        let songs = parse_song_info(&read("music_musicboxsonginfo.bin")?)?;
        let grf = read("music_graffitidata.bin")?;
        let selectable = parse_graffiti_ids(&grf)?;
        let graffiti = Graffiti::parse(&grf)?;
        let selections = selection::parse_selections(&read("music_musicboxselections.bin")?)?;
        let events = EventSystem::parse(&read("music_eventsubscr.bin")?)?;
        let pools = RepetitionPools::parse(&read("music_repetitionpools.bin")?)?;
        let bank_data = read("music_samples.sbr")?;
        let bank = SampleBank::parse(&bank_data)?;
        let sbs_path = find_file(big_root, &rel("music_samples.sbs")).context("music_samples.sbs not found")?;
        let sbs = Arc::new(Bytes::open(&sbs_path)?);
        let mut mb = Self {
            songs,
            selectable,
            selections: vec![selections],
            events,
            graffiti: vec![graffiti],
            pools,
            banks: vec![Bank { bank, data: bank_data, sbs }],
            package: Default::default(),
            package_switches: Default::default(),
            dlc_sets: Vec::new(),
        };
        for set in music_sets(big_root) {
            mb.add_music_set(set);
        }
        Ok(mb)
    }

    fn add_music_set(&mut self, set: DlcMusic) {
        let source = set.source.clone();
        let songs = set.songs.len();
        let mut banks = 0usize;
        for files in &set.banks {
            let read = |p: &Path| -> Result<Vec<u8>> {
                let meta = std::fs::metadata(p).with_context(|| format!("{}", p.display()))?;
                if !meta.is_file() || meta.len() > MAX_BANK_BYTES {
                    bail!("{}: not a file of at most {MAX_BANK_BYTES} bytes", p.display());
                }
                Ok(std::fs::read(p)?)
            };
            let data = match read(&files.sbr) {
                Ok(d) => d,
                Err(e) => {
                    ssx_core::eprintln!(
                        "musicbox: {}: sample bank {}: {}",
                        ssx_core::text::shown_max(&source, 160),
                        ssx_core::text::shown_max(&files.sbr.display().to_string(), 160),
                        ssx_core::text::shown_max(&format!("{e:#}"), 240)
                    );
                    continue;
                }
            };
            let bank = match SampleBank::parse(&data) {
                Ok(b) => b,
                Err(e) => {
                    ssx_core::eprintln!(
                        "musicbox: {}: sample bank {}: {}",
                        ssx_core::text::shown_max(&source, 160),
                        ssx_core::text::shown_max(&files.sbr.display().to_string(), 160),
                        ssx_core::text::shown_max(&format!("{e:#}"), 240)
                    );
                    continue;
                }
            };
            let sbs = match Bytes::open(&files.sbs) {
                Ok(b) => b,
                Err(e) => {
                    ssx_core::eprintln!(
                        "musicbox: {}: stream {}: {}",
                        ssx_core::text::shown_max(&source, 160),
                        ssx_core::text::shown_max(&files.sbs.display().to_string(), 160),
                        ssx_core::text::shown_max(&format!("{e:#}"), 240)
                    );
                    continue;
                }
            };
            self.banks.push(Bank { bank, data, sbs: Arc::new(sbs) });
            banks += 1;
        }
        if banks == 0 {
            ssx_core::eprintln!(
                "musicbox: {}: no sample bank opened; its songs are not loaded",
                ssx_core::text::shown_max(&source, 160)
            );
            return;
        }
        for song in set.songs {
            match self.songs.iter_mut().find(|s| s.id == song.id) {
                Some(old) => *old = song,
                None => self.songs.push(song),
            }
        }
        self.selectable.extend(set.selectable);
        self.graffiti.push(set.graffiti);
        self.selections.push(set.selections);
        for pool in set.pools.pools {
            if self.pools.pool(pool.id).is_none() {
                self.pools.pools.push(pool);
            }
        }
        ssx_core::eprintln!(
            "musicbox: {}: {songs} songs, one Graffiti list, {banks} sample bank(s)",
            ssx_core::text::shown_max(&source, 160)
        );
        self.dlc_sets.push(source);
    }

    pub fn song(&self, id: u32) -> Option<&Song> {
        self.songs.iter().find(|s| s.id == id)
    }

    pub fn set_event_class(&mut self, race: Option<RaceType>) {
        let flag = race.and_then(RaceType::flag);
        for s in self.songs.iter_mut().filter(|s| s.get("Index").is_some()) {
            s.unk[4] = match flag {
                Some(f) if s.flag(f) => 1000,
                Some(_) => 1,
                None => s.default_weight(),
            };
        }
    }

    pub fn apply_song_flags(&mut self, flags: &[(u32, bool)]) {
        for &(id, on) in flags.iter().filter(|(id, _)| *id != 0) {
            if let Some(s) = self.songs.iter_mut().find(|s| s.id == id) {
                s.unk[2] = u32::from(on);
            }
        }
    }

    pub fn duration(&self, id: u32) -> Option<f64> {
        let (bank, offset) = self.sps_offset(id).ok()?;
        let sps = Sps::read(&self.banks.get(bank)?.sbs, offset).ok()?;
        Some(f64::from(sps.header.num_samples) / f64::from(sps.header.sample_rate.max(1)))
    }

    pub fn decode_song(&self, id: u32) -> Result<(Vec<f32>, u32, u16)> {
        let (bank, offset) = self.sps_offset(id)?;
        let sps = Sps::read(&self.banks.get(bank).context("bank gone")?.sbs, offset)?;
        let d = ssx_formats::ealayer3_decode::decode_sound(&sps.header, &sps.blocks)?;
        Ok((d.samples, d.sample_rate, u16::from(d.channels)))
    }

    fn sps_offset(&self, id: u32) -> Result<(usize, usize)> {
        for (i, b) in self.banks.iter().enumerate() {
            let Some(s) = b.bank.sounds.iter().find(|s| s.id == id) else { continue };
            return match b.bank.resolve(&b.data, s)? {
                SoundRef::Sps { sps_offset } => Ok((i, sps_offset as usize)),
                SoundRef::SnrSns { .. } => bail!("sound {id:#x} is not an SPS sound"),
            };
        }
        bail!("no sound {id:#x} in any sample bank")
    }
}

#[derive(Clone, Debug)]
pub struct Playlist {
    pub ids: Vec<u32>,
    pub next: usize,
}

impl Playlist {
    pub fn new(mb: &MusicBox, race: RaceType, seed: u64) -> Self {
        let disc: &[u32] = if mb.package.replace_disc { &[] } else { &mb.selectable };
        let candidates: Vec<u32> = disc
            .iter()
            .chain(&mb.package.added)
            .copied()
            .filter(|&id| race.flag().is_none_or(|f| mb.song(id).is_some_and(|s| s.flag(f))))
            .collect();
        let mut ids = select_levelled(&mb.songs, &candidates);
        let mut x = seed | 1;
        for i in (1..ids.len()).rev() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            ids.swap(i, (x % (i as u64 + 1)) as usize);
        }
        Self { ids, next: 0 }
    }
}

pub const MUSIC_SUBMIX_LEVEL: f32 = 0.5;

static PLAYING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PAUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn playing() -> bool {
    PLAYING.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn paused() -> bool {
    PAUSED.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_paused(on: bool) {
    PAUSED.store(on, std::sync::atomic::Ordering::Relaxed);
}

struct SongStream {
    id: u32,
    source: SongSource,
    buf: Vec<f32>,
    pos: usize,
    gain: f32,
}

enum SongSource {
    Disc(Box<DiscSong>),
    Package(Box<crate::content::audio::stream::MusicStream>),
}

struct DiscSong {
    blocks: Vec<(usize, usize, u32)>,
    next: usize,
    channels: usize,
    dec: Option<(usize, SectionDecoder)>,
    sbs: Arc<Bytes>,
}

const PACKAGE_CHUNK: usize = 1024;

enum Refill {
    More,
    Behind,
    End,
}

pub struct LicensedMusic {
    pub mb: MusicBox,
    pub playlist: Playlist,
    cur: Option<SongStream>,
    tmp: Vec<f32>,
    pub played: Vec<(u32, String)>,
    once: bool,
    game: Option<RacePosts>,
    fade: Option<(f32, f32, u64)>,
}

impl LicensedMusic {
    pub fn new(mb: MusicBox, playlist: Playlist) -> Self {
        Self { mb, playlist, cur: None, tmp: Vec::new(), played: Vec::new(), once: false, game: None, fade: None }
    }

    // TODO(guess): a post's song starts at the check's frame (the game checks on its 60 Hz audio-manager update, then the MusicBox opens the stream with disc latency); settle: the MusicBox's stream start (0x83111DE0 → 0x8310BF18) timed on a console.
    pub fn game(mut mb: MusicBox, race: RaceType) -> Self {
        mb.set_event_class(Some(race));
        let posts = RacePosts::new(RaceEvent::new(race), session(), remix());
        Self { game: Some(posts), ..Self::new(mb, Playlist { ids: Vec::new(), next: 0 }) }
    }

    pub fn menu(mb: MusicBox) -> Self {
        Self::menu_with(mb, session(), remix())
    }

    pub fn menu_with(
        mut mb: MusicBox,
        session: &'static std::sync::Mutex<Session>,
        remix: &'static std::sync::Mutex<RemixClock>,
    ) -> Self {
        mb.set_event_class(None);
        let mut posts = RacePosts::new(RaceEvent::new(RaceType::Menu), session, remix);
        posts.next = posts::first_menu_check(remix.lock().ok().and_then(|r| r.take_over(std::time::Instant::now())));
        Self { game: Some(posts), ..Self::new(mb, Playlist { ids: Vec::new(), next: 0 }) }
    }

    pub fn hold_posts(&mut self, held: bool) {
        if let Some(g) = self.game.as_mut() {
            g.held = held;
        }
    }

    pub fn song_open(&self) -> bool {
        self.cur.is_some()
    }

    pub fn song_id(&self) -> Option<u32> {
        self.cur.as_ref().map(|s| s.id)
    }

    pub fn replace_packages(&mut self, plan: &crate::content::audio::MusicPlan, race: Option<RaceType>) {
        let playing_package = self.cur.as_ref().is_some_and(|s| matches!(s.source, SongSource::Package(_)));
        self.mb.clear_packages();
        self.mb.add_packages(plan);
        self.mb.set_event_class(race);
        if playing_package {
            self.stop(0.5);
        }
    }

    pub fn event_setup(&mut self) {
        let mut remix_of = remix();
        if let Some(g) = self.game.as_mut() {
            let first = (f64::from(crate::sfx_game::tuning::MUSIC_RACE_EVENT_DELAY) * 1000.0) as u64;
            g.next = Some(g.clock + ms_frames(first));
            remix_of = g.remix;
        }
        if let Ok(mut r) = remix_of.lock() {
            r.arm(std::time::Instant::now());
        }
    }

    pub fn candidates(mb: &MusicBox, race: RaceType) -> Vec<(u32, u32)> {
        let ev = RaceEvent::new(race);
        mb.candidates(&ev).into_iter().map(|id| (id, mb.song(id).map_or(0, Song::weight))).collect()
    }

    pub fn song(mb: MusicBox, id: u32) -> Self {
        Self { once: true, ..Self::new(mb, Playlist { ids: vec![id], next: 0 }) }
    }

    pub fn playing(&self) -> bool {
        self.cur.is_some()
            || self.game.is_some()
            || (self.playlist.next < self.playlist.ids.len())
            || (!self.once && !self.playlist.ids.is_empty())
    }

    fn open_song(&mut self, id: u32) -> Option<SongStream> {
        if let Some(a) = self.mb.package_audio(id).cloned() {
            let label = self.mb.song(id).map_or_else(|| format!("song {id:#x}"), Song::label);
            ssx_core::eprintln!("musicbox: {label}: {} from {}", a.file.rel, a.label);
            self.played.push((id, label));
            let stream = crate::content::audio::stream::MusicStream::open(a.file, a.looping);
            let source = SongSource::Package(Box::new(stream));
            return Some(SongStream { id, source, buf: Vec::new(), pos: 0, gain: a.gain });
        }
        let opened = self.mb.sps_offset(id).and_then(|(bank, o)| {
            let b = self.mb.banks.get(bank).context("bank gone")?;
            Ok((b.sbs.clone(), Sps::read(&b.sbs, o)?))
        });
        match opened {
            Ok((sbs, sps)) => {
                let blocks = sps
                    .blocks
                    .sections
                    .iter()
                    .enumerate()
                    .flat_map(|(s, sec)| sec.iter().map(move |b| (b.offset, s, b.samples)))
                    .collect();
                let label = self.mb.song(id).map_or_else(|| format!("sound {id:#x}"), Song::label);
                self.played.push((id, label));
                let channels = sps.header.channels as usize;
                let source = SongSource::Disc(Box::new(DiscSong { blocks, next: 0, channels, dec: None, sbs }));
                Some(SongStream { id, source, buf: Vec::new(), pos: 0, gain: 1.0 })
            }
            Err(e) => {
                ssx_core::eprintln!("musicbox: sound {id:#x}: {e:#}");
                None
            }
        }
    }

    fn open_next(&mut self) -> Option<SongStream> {
        for _ in 0..self.playlist.ids.len() {
            if self.once && self.playlist.next >= self.playlist.ids.len() {
                return None;
            }
            let id = *self.playlist.ids.get(self.playlist.next % self.playlist.ids.len().max(1))?;
            self.playlist.next += 1;
            if let Some(s) = self.open_song(id) {
                return Some(s);
            }
        }
        None
    }

    fn poll(&mut self, now: u64) -> Result<(), u64> {
        let Some(g) = self.game.as_mut() else { return Err(u64::MAX) };
        let Some(next) = g.next.as_mut() else { return Err(u64::MAX) };
        let period = ms_frames(RACE_EVENT_PERIOD_MS);
        if now > *next {
            *next += (now - *next).div_ceil(period) * period;
        }
        if now < *next {
            return Err(*next - now);
        }
        *next = now + period;
        if g.held {
            return Err(period);
        }
        let (event, session) = (g.event, g.session);
        let pick = session.lock().ok().and_then(|mut s| Some((s.select(&self.mb, &event)?, s.seed(), s.picks.len())));
        match pick.and_then(|(id, seed, n)| Some((self.open_song(id)?, seed, n))) {
            Some((s, seed, n)) => {
                let t = now as f64 / f64::from(crate::pathfinder::SAMPLE_RATE);
                let label = self.mb.song(s.id).map_or_else(|| format!("{:#x}", s.id), Song::label);
                ssx_core::eprintln!(
                    "musicbox: music_race_event {event:?} at {t:.2} s → {:#x} {label} (pick {n}, seed {seed:#x})",
                    s.id
                );
                self.cur = Some(s);
                Ok(())
            }
            None => Err(ms_frames(RACE_EVENT_PERIOD_MS)),
        }
    }

    pub fn stop(&mut self, secs: f32) {
        if self.fade.is_some() && secs > 0.0 {
            return;
        }
        if self.cur.is_none() || secs <= 0.0 {
            self.stop_music();
            return;
        }
        let frames = (f64::from(secs) * f64::from(crate::pathfinder::SAMPLE_RATE)).round().max(1.0) as u64;
        self.fade = Some((1.0, 1.0 / frames as f32, frames));
    }

    fn stop_music(&mut self) {
        if let Some(s) = self.cur.take() {
            let label = self.mb.song(s.id).map_or_else(|| format!("{:#x}", s.id), Song::label);
            ssx_core::eprintln!("musicbox: StopMusic ({label})");
        }
        self.fade = None;
    }

    pub fn level_end(&mut self) {
        if let Some(g) = &self.game {
            g.hand_over();
        }
        (self.game, self.playlist.ids) = (None, Vec::new());
        let secs = if paused() { 0.0 } else { crate::sfx_game::tuning::MUSIC_LEVEL_END_FADE };
        self.stop(secs);
    }

    pub fn end_posts(&mut self, secs: f32) {
        if let Some(g) = &self.game {
            g.hand_over();
        }
        self.game = None;
        self.stop(secs);
    }

    pub fn finished(&self) -> bool {
        self.cur.is_none() && self.game.is_none() && self.fade.is_none() && self.playlist.ids.is_empty()
    }

    pub fn render(&mut self, out: &mut [f32], gain: f32) {
        if paused() {
            out.fill(0.0);
            return;
        }
        self.render_songs(out, gain);
        let Some((mut level, step, mut left)) = self.fade else { return };
        let (frames, _) = out.as_chunks_mut::<2>();
        let fading = frames.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        for frame in &mut frames[..fading] {
            level = (level - step).max(0.0);
            frame[0] *= level;
            frame[1] *= level;
        }
        left -= fading as u64;
        self.fade = Some((level, step, left));
        if left == 0 {
            for frame in &mut frames[fading..] {
                *frame = [0.0; 2];
            }
            self.stop_music();
        }
    }

    fn render_songs(&mut self, out: &mut [f32], gain: f32) {
        PLAYING.store(self.cur.is_some(), std::sync::atomic::Ordering::Relaxed);
        let frames = out.len() / 2;
        let start = self.game.as_ref().map_or(0, |g| g.clock);
        if let Some(g) = self.game.as_mut() {
            g.clock += frames as u64;
        }
        let mut f = 0;
        while f < frames {
            if self.cur.is_none() {
                if self.game.is_some() {
                    if let Err(wait) = self.poll(start + f as u64) {
                        let n = wait.min((frames - f) as u64) as usize;
                        out[f * 2..(f + n) * 2].fill(0.0);
                        f += n;
                    }
                    continue;
                }
                self.cur = self.open_next();
                if self.cur.is_none() {
                    out[f * 2..].fill(0.0);
                    return;
                }
            }
            let s = self.cur.as_mut().unwrap();
            if s.pos * 2 >= s.buf.len() {
                s.buf.clear();
                s.pos = 0;
                match Self::refill(s, &mut self.tmp) {
                    Refill::More => {}
                    Refill::Behind => {
                        out[f * 2..].fill(0.0);
                        return;
                    }
                    Refill::End => self.cur = None,
                }
                continue;
            }
            let g = gain * s.gain;
            out[2 * f] = s.buf[2 * s.pos] * g;
            out[2 * f + 1] = s.buf[2 * s.pos + 1] * g;
            s.pos += 1;
            f += 1;
        }
    }

    fn refill(s: &mut SongStream, tmp: &mut Vec<f32>) -> Refill {
        let id = s.id;
        match &mut s.source {
            SongSource::Disc(d) => {
                if d.decode_block(id, &mut s.buf, tmp) {
                    Refill::More
                } else {
                    Refill::End
                }
            }
            SongSource::Package(stream) => {
                s.buf.resize(PACKAGE_CHUNK * 2, 0.0);
                let n = stream.render(&mut s.buf);
                s.buf.truncate(n * 2);
                if n > 0 {
                    return Refill::More;
                }
                match stream.ended() {
                    None => Refill::Behind,
                    Some(Ok(())) => Refill::End,
                    Some(Err(e)) => {
                        ssx_core::eprintln!("musicbox: song {id:#x} ended: {e}");
                        Refill::End
                    }
                }
            }
        }
    }
}

impl DiscSong {
    fn decode_block(&mut self, id: u32, buf: &mut Vec<f32>, tmp: &mut Vec<f32>) -> bool {
        let Some(&(offset, section, samples)) = self.blocks.get(self.next) else { return false };
        self.next += 1;
        if self.dec.as_ref().is_none_or(|(x, _)| *x != section) {
            match SectionDecoder::new(self.channels as u8) {
                Ok(d) => self.dec = Some((section, d)),
                Err(_) => return false,
            }
        }
        let Ok(block) = ssx_formats::eaac::Block::read(&self.sbs, offset) else { return false };
        tmp.clear();
        let Some((_, dec)) = self.dec.as_mut() else { return false };
        let Ok(n) = dec.decode_payload(block.payload, tmp) else {
            ssx_core::eprintln!("musicbox: sound {id:#x}: undecodable block at {offset:#x}");
            return false;
        };
        let ch = self.channels;
        for k in 0..n.min(samples as usize) {
            if ch == 1 {
                buf.extend_from_slice(&[tmp[k], tmp[k]]);
            } else {
                buf.extend_from_slice(&tmp[k * ch..k * ch + 2]);
            }
        }
        true
    }
}

impl LicensedMusic {
    pub fn now_playing(&self) -> Option<String> {
        self.cur.as_ref().and_then(|s| self.mb.song(s.id)).map(Song::label)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: u32, level: u32, weight: u32) -> Song {
        Song { id, unk: [1, 1, level, 100, weight, 4], info: Vec::new() }
    }

    fn one_song(count: usize) -> Vec<u8> {
        let mut d = b"MBSI".to_vec();
        for v in [0x0002_0002u32, 0, 0x14, 0, u32::try_from(count).unwrap(), 0x1c] {
            d.extend(v.to_be_bytes());
        }
        d.extend(0xBEu32.to_be_bytes());
        for v in [1u32, 1, 1, 100, 100, 4, 1, 0x40] {
            d.extend(v.to_be_bytes());
        }
        for v in [0x48u32, 0x4E] {
            d.extend(v.to_be_bytes());
        }
        d.extend(b"Index\0");
        d.extend(b"101\0");
        d
    }

    #[test]
    fn the_song_tables_header_is_the_record_count() {
        let songs = parse_song_info(&one_song(1)).unwrap();
        assert_eq!(songs.len(), 1);
        assert_eq!((songs[0].id, songs[0].get("Index")), (0xBE, Some("101")));
        assert!(parse_song_info(&one_song(2)).is_err());
    }

    #[test]
    fn a_song_switched_off_is_never_a_candidate() {
        let songs = vec![song(1, 1, 100), song(2, 0, 100), song(3, 1, 100), song(4, 1, 0)];
        assert_eq!(select_levelled(&songs, &[1, 2, 3, 4]), [1, 3], "level 0 and weight 0 are left out");
        let off = vec![song(1, 0, 100), song(2, 0, 100)];
        assert!(select_levelled(&off, &[1, 2]).is_empty(), "the level starts at 1: all off plays nothing");
    }

    #[test]
    fn the_highest_level_wins_and_level_3_ends_the_scan() {
        let songs = vec![song(1, 1, 100), song(2, 2, 100), song(3, 1, 100), song(4, 2, 100), song(5, 3, 100)];
        assert_eq!(select_levelled(&songs, &[1, 2, 3, 4]), [2, 4]);
        assert_eq!(select_levelled(&songs, &[1, 5, 2]), [5]);
    }
}
