use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use bevy::asset::RenderAssetUsages;
use bevy::prelude::{Assets, Handle, Image, warn};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ssx_assets::sfx::movie::MovieAudio;
use ssx_formats::ea_movie::{AudioDecoder, Movie, VideoHeader};
use ssx_formats::vp6::Decoder;

type FrameMsg = (usize, Vec<u8>);

pub struct Vp6Movie {
    rx: Mutex<Receiver<FrameMsg>>,
    next: Option<FrameMsg>,
    pub image: Handle<Image>,
    pub fps: f64,
    pub frames: usize,
    pub time: f64,
    done: bool,
    pub shown: Option<usize>,
    audio: Option<Arc<MovieAudio>>,
    stopwatch: f64,
}

fn decode_video(data: &[u8], tx: &SyncSender<FrameMsg>) {
    let Ok(movie) = Movie::parse(data) else { return };
    let mut dec = Decoder::new();
    for (i, f) in movie.frames.iter().enumerate() {
        if dec.decode_frame(f.data).is_err() {
            return;
        }
        let Some(pic) = dec.picture() else { return };
        let mut rgba = Vec::new();
        pic.write_rgba(&mut rgba);
        if tx.send((i, rgba)).is_err() {
            return;
        }
    }
}

const AUDIO_AHEAD: f64 = 1.0;

fn spawn_audio(data: Arc<Vec<u8>>, lang: Option<[u8; 2]>, feed: Arc<MovieAudio>) {
    std::thread::spawn(move || {
        let Ok(movie) = Movie::parse(&data) else { return feed.finish() };
        let Some(stream) = movie.audio_stream(lang) else { return feed.finish() };
        let Ok(mut dec) = AudioDecoder::new(&stream.header, false) else { return feed.finish() };
        let ahead = (AUDIO_AHEAD * f64::from(dec.sample_rate)) as usize;
        let mut pcm = Vec::new();
        for b in &stream.blocks {
            while feed.queued_frames() >= ahead {
                if feed.stopped() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            pcm.clear();
            if let Err(e) = dec.decode_block(b, &mut pcm) {
                warn!("movie audio: {e}");
                break;
            }
            feed.push(&pcm);
        }
        feed.finish();
    });
}

#[allow(dead_code)]
pub fn movie_language(lang: &str) -> Option<[u8; 2]> {
    let l = lang.to_ascii_lowercase();
    let index = match l.get(..2).unwrap_or("") {
        "fr" => 1,
        "ge" | "de" => 2,
        "sp" | "es" => 3,
        "it" => 4,
        "jp" | "ja" => 5,
        _ => 0,
    };
    ssx_formats::ea_movie::language_stream(index)
}

impl Vp6Movie {
    pub fn open_lang(name: &str, paths: &[PathBuf], images: &mut Assets<Image>, lang: Option<[u8; 2]>) -> Result<Self> {
        let path = paths.iter().find(|p| p.is_file()).with_context(|| format!("movie {name} not found"))?;
        let head = read_head(path)?;
        let v = VideoHeader::parse(&head[8..32]).context("the movie's MVhd")?;
        let size = Extent3d { width: u32::from(v.width), height: u32::from(v.height), depth_or_array_layers: 1 };
        let black = vec![0u8; v.width as usize * v.height as usize * 4];
        let image = images.add(Image::new(
            size,
            TextureDimension::D2,
            black,
            TextureFormat::Rgba8UnormSrgb,
            RenderAssetUsages::default(),
        ));
        let audio = audio_header(&head, lang).map(|(ch, rate)| MovieAudio::new(ch, rate));
        let (tx, rx) = sync_channel::<FrameMsg>(4);
        let (file, feed) = (path.clone(), audio.clone());
        std::thread::spawn(move || {
            let Ok(data) = std::fs::read(&file).map(Arc::new) else {
                if let Some(f) = &feed {
                    f.finish();
                }
                return;
            };
            if let Some(f) = feed {
                spawn_audio(data.clone(), lang, f);
            }
            decode_video(&data, &tx);
        });
        Ok(Vp6Movie {
            rx: Mutex::new(rx),
            next: None,
            image,
            fps: v.fps(),
            frames: v.frame_count as usize,
            time: 0.0,
            done: false,
            shown: None,
            audio,
            stopwatch: 0.0,
        })
    }

    #[allow(dead_code)]
    pub fn audio(&self) -> Option<Arc<MovieAudio>> {
        self.audio.clone()
    }

    fn clock(&mut self, dt: f64) -> (f64, bool) {
        self.stopwatch += dt;
        match &self.audio {
            Some(a) if a.attached() => match a.played_seconds() {
                Some(t) => (t.min(self.stopwatch), true),
                None if self.stopwatch < super::VOICE_WAIT => (0.0, true),
                None => (self.stopwatch - super::VOICE_WAIT, false),
            },
            _ => (self.stopwatch, false),
        }
    }

    pub fn advance(&mut self, dt: f64, images: &mut Assets<Image>) -> bool {
        let (t, voice) = self.clock(dt);
        self.time = t;
        let ms = (t * 1000.0) as f32;
        let due = (ms * (self.fps as f32 * 0.001)) as usize;
        let target = due.min(self.frames.saturating_sub(1));
        let mut latest = None;
        loop {
            if self.next.is_none() {
                let behind = self.shown.is_none_or(|s| s < target);
                let r = self
                    .rx
                    .lock()
                    .map(|rx| if behind { rx.recv().map_err(|_| TryRecvError::Disconnected) } else { rx.try_recv() })
                    .unwrap_or(Err(TryRecvError::Disconnected));
                match r {
                    Ok(m) => self.next = Some(m),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.done = true;
                        break;
                    }
                }
            }
            if self.next.as_ref().is_some_and(|(i, _)| *i <= target) {
                latest = self.next.take();
            } else {
                break;
            }
        }
        if let Some((i, rgba)) = latest {
            if let Some(mut img) = images.get_mut(&self.image) {
                img.data = Some(rgba);
            }
            self.shown = Some(i);
        }
        let last_shown = self.done || self.shown.is_some_and(|s| s + 1 >= self.frames);
        let audio_over = voice && self.audio.as_ref().is_some_and(|a| a.is_over());
        last_shown && (t * self.fps >= self.frames as f64 || audio_over)
    }
}

impl Drop for Vp6Movie {
    fn drop(&mut self) {
        if let Some(a) = &self.audio {
            a.stop();
        }
    }
}

fn audio_header(head: &[u8], lang: Option<[u8; 2]>) -> Option<(usize, u32)> {
    let mut found = None;
    let mut pos = 0;
    while let Some(c) = head.get(pos..pos + 8) {
        let size = u32::from_le_bytes([c[4], c[5], c[6], c[7]]) as usize;
        let tag = [c[0], c[1], c[2], c[3]];
        let this = match &tag {
            b"SCHl" => Some(None),
            [b'S', b'H', a, b] => Some(Some([*a, *b])),
            _ => None,
        };
        if let Some(l) = this
            && let Some(body) = head.get(pos + 8..pos + size.max(8))
            && let Ok(h) = ssx_formats::ea_movie::AudioHeader::parse(body)
            && (l == lang || (l.is_none() && found.is_none()))
        {
            found = Some((h.channels as usize, h.sample_rate));
        }
        if &tag == b"MV0K" || size < 8 {
            break;
        }
        pos += size;
    }
    found
}

fn read_head(path: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut head = Vec::new();
    f.take(65_536).read_to_end(&mut head)?;
    if head.len() < 32 {
        anyhow::bail!("{}: too short", path.display());
    }
    Ok(head)
}
