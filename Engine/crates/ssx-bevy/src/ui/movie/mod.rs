mod vp6;
mod webm;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use bevy::prelude::{Assets, Handle, Image};
use ssx_assets::content::GameContent;
use ssx_assets::content::fmv::Fmv;
use ssx_assets::sfx::movie::MovieAudio;

use vp6::Vp6Movie;
pub use vp6::movie_language;
use webm::WebmMovie;

// PORT(choice): a voice waits this long for its first block before the movie runs on its stopwatch (an app without an
// output never runs the voice; while the output runs the voice starts within one output pull, ~21 ms).
const VOICE_WAIT: f64 = 0.5;

enum Inner {
    Disc(Vp6Movie),
    Package(Box<WebmMovie>),
}

pub struct MoviePlayback {
    pub name: String,
    pub image: Handle<Image>,
    pub fps: f64,
    pub frames: usize,
    pub time: f64,
    inner: Inner,
}

impl MoviePlayback {
    pub fn open(name: &str, paths: &[PathBuf], images: &mut Assets<Image>) -> Result<Self> {
        Self::open_lang(name, paths, images, None)
    }

    pub fn open_lang(name: &str, paths: &[PathBuf], images: &mut Assets<Image>, lang: Option<[u8; 2]>) -> Result<Self> {
        Ok(Self::disc(name, Vp6Movie::open_lang(name, paths, images, lang)?))
    }

    pub fn open_fmv(
        name: &str,
        fmv: &Fmv,
        content: &GameContent,
        images: &mut Assets<Image>,
        lang: Option<[u8; 2]>,
    ) -> Result<Self> {
        match fmv {
            Fmv::Disc(path) => Self::open_lang(name, std::slice::from_ref(path), images, lang),
            Fmv::Package { .. } => {
                let m = WebmMovie::open(fmv, content, images)?;
                let image = m.image.clone();
                Ok(Self {
                    name: name.to_string(),
                    image,
                    fps: 0.0,
                    frames: 0,
                    time: 0.0,
                    inner: Inner::Package(Box::new(m)),
                })
            }
        }
    }

    fn disc(name: &str, m: Vp6Movie) -> Self {
        let (image, fps, frames) = (m.image.clone(), m.fps, m.frames);
        Self { name: name.to_string(), image, fps, frames, time: 0.0, inner: Inner::Disc(m) }
    }

    pub fn describe(&self) -> String {
        match &self.inner {
            Inner::Disc(_) => format!("{} frames at {:.2} fps", self.frames, self.fps),
            Inner::Package(m) => m.describe(),
        }
    }

    pub fn frame(&self) -> i32 {
        match &self.inner {
            Inner::Disc(_) => (self.time * self.fps) as i32,
            Inner::Package(m) => m.frame().map_or(0, |f| i32::try_from(f).unwrap_or(i32::MAX)),
        }
    }

    pub fn audio(&self) -> Option<Arc<MovieAudio>> {
        match &self.inner {
            Inner::Disc(m) => m.audio(),
            Inner::Package(m) => m.audio(),
        }
    }

    pub fn advance(&mut self, dt: f64, images: &mut Assets<Image>) -> bool {
        let over = match &mut self.inner {
            Inner::Disc(m) => m.advance(dt, images),
            Inner::Package(m) => m.advance(dt, images),
        };
        self.time = match &self.inner {
            Inner::Disc(m) => m.time,
            Inner::Package(m) => m.time,
        };
        over
    }

    pub fn failure(&self) -> Option<(&ssx_content::ContentId, &str, &str)> {
        match &self.inner {
            Inner::Package(m) => m.failure.as_deref().map(|f| (&m.id, m.source.as_str(), f)),
            Inner::Disc(_) => None,
        }
    }

    pub fn picture_counts(&self) -> Option<String> {
        match &self.inner {
            Inner::Package(m) => Some(m.picture_counts()),
            Inner::Disc(_) => None,
        }
    }
}

pub struct LoopingMovie(Box<WebmMovie>);

pub struct LoopingStart(webm::Started);

impl LoopingMovie {
    pub fn start(fmv: &Fmv, content: &GameContent) -> Result<LoopingStart> {
        WebmMovie::start_looping(fmv, content).map(LoopingStart)
    }

    pub fn show(start: LoopingStart, images: &mut Assets<Image>) -> Self {
        Self(Box::new(WebmMovie::looping(start.0, images)))
    }

    pub fn picture(&self) -> Option<&Handle<Image>> {
        self.0.frame().map(|_| &self.0.image)
    }

    pub fn advance(&mut self, dt: f64, images: &mut Assets<Image>) -> bool {
        self.0.advance(dt, images)
    }

    pub fn failure(&self) -> Option<(&ssx_content::ContentId, &str, &str)> {
        self.0.failure.as_deref().map(|f| (&self.0.id, self.0.source.as_str(), f))
    }

    pub fn describe(&self) -> String {
        self.0.describe()
    }

    pub fn picture_counts(&self) -> String {
        self.0.picture_counts()
    }
}
