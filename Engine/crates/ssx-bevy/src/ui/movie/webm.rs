use std::sync::Arc;

use anyhow::Result;
use bevy::asset::RenderAssetUsages;
use bevy::prelude::{Assets, Handle, Image};
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use ssx_assets::content::GameContent;
use ssx_assets::content::fmv::Fmv;
use ssx_assets::sfx::movie::MovieAudio;
use ssx_content::movie::Limits;
use ssx_movie::Threads;
use ssx_movie::play::{AudioSink, Playback};

const STALL_S: f64 = 10.0;

pub struct WebmMovie {
    play: Playback,
    pub image: Handle<Image>,
    pub time: f64,
    audio: Option<Arc<MovieAudio>>,
    stopwatch: f64,
    voice_end: Option<(f64, f64)>,
    since_picture: f64,
    frames_seen: u64,
    pub failure: Option<String>,
    pub id: ssx_content::ContentId,
    pub source: String,
}

pub fn decoder_threads() -> Threads {
    let n = std::thread::available_parallelism().map_or(2, |n| n.get());
    Threads { threads: (n.saturating_sub(1)).clamp(1, 8) as u16, frame_delay: 0 }
}

pub struct Started {
    play: Playback,
    id: ssx_content::ContentId,
    source: String,
}

impl WebmMovie {
    pub fn open(fmv: &Fmv, content: &GameContent, images: &mut Assets<Image>) -> Result<Self> {
        let Fmv::Package { id, source, info, .. } = fmv else { anyhow::bail!("not a package movie") };
        let (file, len) = content.open_fmv(fmv).map_err(anyhow::Error::msg)?;
        let audio = info.audio.as_ref().map(|a| MovieAudio::new(usize::from(a.channels), a.rate));
        let sink: Arc<dyn AudioSink> = match &audio {
            Some(a) => a.clone(),
            None => Arc::new(Silent),
        };
        let play =
            Playback::start(file, len, &Limits::default(), decoder_threads(), sink).map_err(anyhow::Error::msg)?;
        let started = Started { play, id: id.clone(), source: source.clone() };
        Ok(Self::shown(started, audio, TextureFormat::Rgba8UnormSrgb, images))
    }

    pub fn start_looping(fmv: &Fmv, content: &GameContent) -> Result<Started> {
        let Fmv::Package { id, source, .. } = fmv else { anyhow::bail!("not a package movie") };
        let (file, len) = content.open_fmv(fmv).map_err(anyhow::Error::msg)?;
        let play =
            Playback::start_looping(file, len, &Limits::default(), decoder_threads()).map_err(anyhow::Error::msg)?;
        Ok(Started { play, id: id.clone(), source: source.clone() })
    }

    pub fn looping(started: Started, images: &mut Assets<Image>) -> Self {
        Self::shown(started, None, TextureFormat::Rgba8Unorm, images)
    }

    fn shown(
        started: Started,
        audio: Option<Arc<MovieAudio>>,
        format: TextureFormat,
        images: &mut Assets<Image>,
    ) -> Self {
        let Started { play, id, source } = started;
        let v = &play.info.video;
        let size = Extent3d { width: v.width, height: v.height, depth_or_array_layers: 1 };
        let black = vec![0u8; v.width as usize * v.height as usize * 4];
        let image = images.add(Image::new(size, TextureDimension::D2, black, format, RenderAssetUsages::default()));
        Self {
            play,
            image,
            time: 0.0,
            audio,
            stopwatch: 0.0,
            voice_end: None,
            since_picture: 0.0,
            frames_seen: 0,
            failure: None,
            id,
            source,
        }
    }

    pub fn audio(&self) -> Option<Arc<MovieAudio>> {
        self.audio.clone()
    }

    fn clock(&mut self, dt: f64) -> f64 {
        self.stopwatch += dt;
        let Some(a) = self.audio.as_ref().filter(|a| a.attached()) else {
            if self.stopwatch >= super::VOICE_WAIT {
                self.play.mute();
            }
            return self.stopwatch;
        };
        if let Some((t, at)) = self.voice_end {
            return t + (self.stopwatch - at);
        }
        match a.played_seconds() {
            Some(t) if a.is_over() => {
                self.voice_end = Some((t, self.stopwatch));
                t
            }
            Some(t) => t.min(self.stopwatch),
            None if self.stopwatch < super::VOICE_WAIT => 0.0,
            None => {
                self.play.mute();
                self.stopwatch - super::VOICE_WAIT
            }
        }
    }

    pub fn advance(&mut self, dt: f64, images: &mut Assets<Image>) -> bool {
        let t = self.clock(dt);
        self.time = t;
        self.frames_seen += 1;
        if self.failure.is_some() {
            return true;
        }
        let received = self.play.received;
        if let Some(f) = self.play.due((t * 1e9) as i64)
            && let Some(mut img) = images.get_mut(&self.image)
        {
            img.data = Some(f.rgba);
        }
        if self.play.received != received || !self.play.waiting() {
            self.since_picture = 0.0;
        } else {
            self.since_picture += dt;
        }
        match self.play.ended() {
            Some(Ok(())) => self.audio.as_ref().is_none_or(|a| !a.attached() || a.is_over()),
            Some(Err(e)) => {
                self.failure = Some(e.clone());
                true
            }
            None if self.since_picture > STALL_S => {
                self.failure = Some(format!("no picture came for {STALL_S:.0} s (the picture decoder stopped)"));
                true
            }
            None => false,
        }
    }

    pub fn frame(&self) -> Option<u64> {
        self.play.frame
    }

    pub fn describe(&self) -> String {
        let i = &self.play.info;
        let sound = i.audio.as_ref().map_or("no sound".into(), |a| format!("Vorbis {} ch {} Hz", a.channels, a.rate));
        format!(
            "{} of {}: AV1 {} × {}, {:.3} s, {sound}",
            self.id,
            self.source,
            i.video.width,
            i.video.height,
            i.duration_seconds()
        )
    }

    pub fn picture_counts(&self) -> String {
        let p = &self.play;
        let rate = self.frames_seen as f64 / self.time.max(1e-3);
        format!(
            "{} pictures shown, {} passed over; {} frames of the game ({rate:.1} a second), at {} of them the \
             decoder had no picture ahead",
            p.shown, p.dropped, self.frames_seen, p.waits
        )
    }
}

impl Drop for WebmMovie {
    fn drop(&mut self) {
        if let Some(a) = &self.audio {
            a.stop();
        }
    }
}

struct Silent;

impl AudioSink for Silent {
    fn push(&self, _frames: &[f32]) {}
    fn finish(&self) {}
    fn queued_frames(&self) -> usize {
        0
    }
    fn stopped(&self) -> bool {
        false
    }
}
