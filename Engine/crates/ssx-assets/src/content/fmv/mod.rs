pub mod disc;
pub mod export;
pub mod ffmpeg;
pub mod port;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use ssx_content::movie::MovieInfo;
use ssx_content::package::PackageMovieHead;
use ssx_content::{Asset, ContentId, Kind, Layer, Registry};

use super::GameContent;
use super::store::{Shared, provides_kind};
pub use disc::{DiscMovie, DiscMovies};

#[derive(Default)]
pub struct FmvCache {
    disc: OnceLock<Option<Arc<DiscMovies>>>,
    mounted: Mutex<bool>,
    decided: Mutex<BTreeMap<(ContentId, u64), Option<Fmv>>>,
    refused: Mutex<BTreeMap<(ContentId, String), String>>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fmv {
    Disc(std::path::PathBuf),
    Package { id: ContentId, source: String, layer: Layer, rel: String, len: u64, info: Arc<MovieInfo> },
}

impl Fmv {
    pub fn seconds(&self) -> Option<f64> {
        match self {
            Fmv::Disc(path) => disc_info(path).map(|(frames, fps)| f64::from(frames) / fps),
            Fmv::Package { info, .. } => Some(info.duration_seconds()),
        }
    }

    pub fn fps(&self) -> Option<f64> {
        match self {
            Fmv::Disc(path) => disc_info(path).map(|(_, fps)| fps),
            Fmv::Package { info, .. } => Some(info.video.default_duration_ns.map_or(30.0, |d| 1e9 / d.max(1) as f64)),
        }
    }
}

pub fn disc_info(path: &std::path::Path) -> Option<(u32, f64)> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 32];
    f.read_exact(&mut head).ok()?;
    let v = ssx_formats::ea_movie::VideoHeader::parse(&head[8..]).ok()?;
    Some((v.frame_count, v.fps()))
}

pub fn movie_id(name: &str) -> Option<ContentId> {
    if name.contains(':') {
        return name.parse::<ContentId>().ok().filter(|id| id.kind() == Kind::Movie);
    }
    ContentId::new("ssx", Kind::Movie, name).ok()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl GameContent {
    pub(in crate::content) fn disc_movies(&self) -> Option<Arc<DiscMovies>> {
        self.fmv
            .disc
            .get_or_init(|| match DiscMovies::index(self.big_root()) {
                Ok(d) => Some(Arc::new(d)),
                Err(e) => {
                    self.notice(format!("content: the disc's movies under {}: {e:#}", self.big_root().display()));
                    None
                }
            })
            .clone()
    }

    pub(super) fn mount_fmv(&self, registry: &mut Registry) -> bool {
        let mut mounted = lock(&self.fmv.mounted);
        if *mounted || !self.mount_fmv_preview(registry) {
            return false;
        }
        *mounted = true;
        true
    }

    pub(super) fn mount_fmv_preview(&self, registry: &mut Registry) -> bool {
        if !provides_kind(registry, Kind::Movie) {
            return false;
        }
        registry.mount(Layer::Port, 0, Box::new(port::PortMovies));
        if let Some(d) = self.disc_movies() {
            registry.mount(Layer::Base, 0, Box::new(Shared(d)));
        }
        true
    }
}

impl GameContent {
    pub fn fmv(&self, id: &ContentId) -> Option<Fmv> {
        if !self.packages_provide(Kind::Movie) {
            return self.disc_movies().and_then(|d| d.files.get(id).cloned()).map(Fmv::Disc);
        }
        let generation = self.generation();
        let key = (id.clone(), generation);
        if let Some(hit) = lock(&self.fmv.decided).get(&key) {
            return hit.clone();
        }
        let refused = lock(&self.fmv.refused).clone();
        let taken = self.with_registry(|r| {
            for (m, src) in r.providers(id) {
                if let Some(why) = refused.get(&(id.clone(), m.name.to_string())) {
                    self.passed_over(id, m.name, m.layer, why, Some("the next source"));
                    continue;
                }
                match src.load(id) {
                    Ok(Asset::Value(v)) => {
                        if v.downcast_ref::<port::EmptySlot>().is_some() {
                            return None;
                        }
                        if let Some(d) = v.downcast_ref::<DiscMovie>() {
                            return Some(Fmv::Disc(d.path.clone()));
                        }
                        if let Some(h) = v.downcast_ref::<PackageMovieHead>() {
                            let (rel, len, info) = (h.rel.clone(), h.len, h.info.clone());
                            return Some(Fmv::Package {
                                id: id.clone(),
                                source: m.name.to_string(),
                                layer: m.layer,
                                rel,
                                len,
                                info,
                            });
                        }
                    }
                    Ok(_) => {}
                    Err(e) => self.passed_over(id, m.name, m.layer, &e.to_string(), Some("the next source")),
                }
            }
            None
        });
        if let Some(Fmv::Package { source, rel, info, .. }) = &taken {
            self.info(format!(
                "content: {id} plays {rel} of {source}: AV1 {} × {}, {:.3} s{}",
                info.video.width,
                info.video.height,
                info.duration_seconds(),
                info.audio
                    .as_ref()
                    .map_or(", no sound".into(), |a| format!(", Vorbis {} channels at {} Hz", a.channels, a.rate))
            ));
        }
        let mut decided = lock(&self.fmv.decided);
        decided.retain(|(_, g), _| *g == generation);
        decided.insert(key, taken.clone());
        taken
    }

    pub fn open_fmv(&self, fmv: &Fmv) -> Result<(std::fs::File, u64), String> {
        let Fmv::Package { id, source, layer, len, .. } = fmv else {
            return Err("the disc's movies are played from their files".into());
        };
        let (file, size) = self.with_registry(|r| {
            let (_, src) = r
                .providers(id)
                .find(|(m, _)| m.name == source && m.layer == *layer)
                .ok_or_else(|| format!("{id}: {source} is no longer mounted"))?;
            src.movie_file(id)
                .unwrap_or_else(|| Err(ssx_content::LoadError::Missing(id.clone())))
                .map_err(|e| e.to_string())
        })?;
        if size != *len {
            return Err(format!("{id}: its file changed since it was read ({len} bytes then, {size} now)"));
        }
        Ok((file, size))
    }

    pub fn refuse_fmv(&self, id: &ContentId, source: &str, why: &str) {
        lock(&self.fmv.refused).insert((id.clone(), source.to_string()), why.to_string());
        lock(&self.fmv.decided).retain(|(i, _), _| i != id);
        self.notice(format!("content: {id} of {source} stopped: {why}; the next source plays from its next request"));
    }
}

pub fn dev_problem(source: &dyn ssx_content::Source, id: &ContentId) -> Option<String> {
    let limits = ssx_content::movie::Limits::default();
    let file = source.movie_file(id)?;
    let walked = file
        .map_err(|e| e.to_string())
        .and_then(|(f, len)| ssx_content::movie::walk(f, len, &limits).map_err(|e| format!("{id}: {e}")));
    walked.err()
}

#[cfg(test)]
mod tests;
