use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};

use ssx_formats::gfx::{Movie, Tag};

use super::library::movie_key;
use crate::fs::{FE_ROOT, FileSource};
use crate::image::{self, ImageData};

pub const MAX_FILES: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Inputs {
    overlays: Vec<PathBuf>,
    title_update: Option<PathBuf>,
    content: Option<u64>,
}

impl Inputs {
    fn now(fs: &dyn FileSource) -> Self {
        Inputs {
            overlays: ssx_assets::data_roots::overlays(),
            title_update: ssx_assets::data_roots::title_update(),
            content: fs.content().map(|c| c.generation()),
        }
    }
}

pub struct Ahead {
    inputs: Inputs,
    images: Arc<Mutex<HashMap<String, ImageData>>>,
}

pub fn is_movie(data: &[u8]) -> bool {
    matches!(data.get(..3), Some(b"CFX" | b"GFX" | b"FWS" | b"CWS"))
}

fn decode(fs: &dyn FileSource, file: &str) -> Option<ImageData> {
    if !fs.is_content_image(file) && fs.read(file).is_none_or(|d| is_movie(&d)) {
        return None;
    }
    image::load(fs, file).ok()
}

impl Ahead {
    pub fn start(fs: Arc<dyn FileSource>, mut files: Vec<String>) -> std::io::Result<Self> {
        files.truncate(MAX_FILES);
        let inputs = Inputs::now(fs.as_ref());
        let images = Arc::new(Mutex::new(HashMap::new()));
        let out = images.clone();
        std::thread::Builder::new().name("ui images ahead".into()).spawn(move || {
            for file in files {
                if let Some(img) = decode(fs.as_ref(), &file) {
                    out.lock().unwrap_or_else(PoisonError::into_inner).insert(file, img);
                }
            }
        })?;
        Ok(Ahead { inputs, images })
    }

    pub fn is_current(&self, fs: &dyn FileSource) -> bool {
        self.inputs == Inputs::now(fs)
    }

    pub fn take(&self, file: &str) -> Option<ImageData> {
        self.images.lock().unwrap_or_else(PoisonError::into_inner).remove(file)
    }
}

pub const MAX_MOVIES: usize = 64;

pub struct MoviesAhead {
    inputs: Inputs,
    found: Arc<Mutex<Found>>,
}

#[derive(Default)]
struct Found {
    movies: HashMap<String, Movie>,
    images: HashMap<String, ImageData>,
}

impl MoviesAhead {
    pub fn start(fs: Arc<dyn FileSource>, keys: Vec<String>, held: HashSet<String>) -> std::io::Result<Self> {
        let inputs = Inputs::now(fs.as_ref());
        let found = Arc::new(Mutex::new(Found::default()));
        let out = found.clone();
        std::thread::Builder::new()
            .name("ui movies ahead".into())
            .spawn(move || walk(fs.as_ref(), keys, held, &out))?;
        Ok(MoviesAhead { inputs, found })
    }

    pub fn is_current(&self, fs: &dyn FileSource) -> bool {
        self.inputs == Inputs::now(fs)
    }

    pub fn take_movie(&self, file: &str) -> Option<Movie> {
        self.found.lock().unwrap_or_else(PoisonError::into_inner).movies.remove(file)
    }

    pub fn take_image(&self, path: &str) -> Option<ImageData> {
        self.found.lock().unwrap_or_else(PoisonError::into_inner).images.remove(path)
    }
}

fn walk(fs: &dyn FileSource, keys: Vec<String>, held: HashSet<String>, out: &Mutex<Found>) {
    let mut walk = Walk { seen: held, walked: 0 };
    for key in keys {
        walk_from(fs, key, &mut walk, out);
    }
}

struct Walk {
    seen: HashSet<String>,
    walked: usize,
}

fn walk_from(fs: &dyn FileSource, key: String, walk: &mut Walk, out: &Mutex<Found>) {
    let mut queue = std::collections::VecDeque::from([key]);
    while let Some(key) = queue.pop_front() {
        if walk.walked >= MAX_MOVIES || !walk.seen.insert(key.clone()) {
            continue;
        }
        walk.walked += 1;
        let file = format!("{FE_ROOT}{key}");
        if let Some(img) = decode(fs, &file) {
            out.lock().unwrap_or_else(PoisonError::into_inner).images.insert(file, img);
            continue;
        }
        let Some(data) = fs.read(&file) else { continue };
        let Ok(movie) = Movie::parse(&data) else { continue };
        let dir = file.rsplit_once('/').map_or(String::new(), |(d, _)| format!("{d}/"));
        let mut imports: Vec<(u16, String)> = Vec::new();
        for t in &movie.tags {
            match &t.tag {
                Tag::ExternalImage(e) => {
                    let Some(path) = image::resolve_external(fs, &dir, &e.file_name) else { continue };
                    if let Ok(img) = image::load(fs, &path) {
                        out.lock().unwrap_or_else(PoisonError::into_inner).images.entry(path).or_insert(img);
                    }
                }
                Tag::ImportAssets { url, assets, .. } => {
                    imports.extend(assets.iter().map(|(cid, _)| (*cid, url.clone())));
                }
                _ => {}
            }
        }
        out.lock().unwrap_or_else(PoisonError::into_inner).movies.insert(file, movie);
        imports.sort();
        queue.extend(imports.into_iter().map(|(_, url)| movie_key(&url)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Result, bail};

    struct Files;

    impl FileSource for Files {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            match path {
                "a.gfx" => Some(b"GFX\x08".to_vec()),
                "b.dds" | "c.dds" => Some(b"DDS ".to_vec()),
                _ => None,
            }
        }
        fn exists(&self, path: &str) -> bool {
            self.read(path).is_some()
        }
        fn list(&self, _: &str, _: &str) -> Vec<String> {
            Vec::new()
        }
        fn image(&self, path: &str) -> Result<ImageData> {
            if path != "b.dds" {
                bail!("{path}: no image");
            }
            Ok(ImageData { width: 1, height: 1, rgba: vec![1, 2, 3, 4], source: path.into() })
        }
    }

    #[test]
    fn decodes_the_images_a_load_would_make_and_hands_each_out_once() {
        let files = ["a.gfx", "b.dds", "c.dds", "d.dds"].map(String::from).to_vec();
        let ahead = Ahead::start(Arc::new(Files), files).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let b = loop {
            if let Some(img) = ahead.take("b.dds") {
                break img;
            }
            assert!(std::time::Instant::now() < deadline, "b.dds not decoded");
            std::thread::yield_now();
        };
        assert_eq!(b.rgba, [1, 2, 3, 4]);
        assert!(ahead.take("b.dds").is_none());
        for f in ["a.gfx", "c.dds", "d.dds"] {
            assert!(ahead.take(f).is_none(), "{f}");
        }
        assert!(ahead.is_current(&Files));
        assert!(is_movie(b"CWS") && is_movie(b"FWS\x0a") && !is_movie(b"DDS ") && !is_movie(b"GF"));
    }

    fn swf(tags: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let mut body = vec![0u8, 0, 12, 1, 0];
        for (code, data) in tags {
            body.extend_from_slice(&((code << 6) | data.len() as u16).to_le_bytes());
            body.extend_from_slice(data);
        }
        body.extend_from_slice(&[0, 0]);
        let mut out = b"FWS\x08".to_vec();
        out.extend_from_slice(&(8 + body.len() as u32).to_le_bytes());
        out.extend(body);
        out
    }

    fn import(url: &str, name: &str) -> (u16, Vec<u8>) {
        let mut b = url.as_bytes().to_vec();
        b.push(0);
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(name.as_bytes());
        b.push(0);
        (57, b)
    }

    struct Movies;

    impl FileSource for Movies {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            match path {
                "data/common/fe/views/a.gfx" => Some(swf(&[import("parts.swf", "x"), import("icon.dds", "y")])),
                "data/common/fe/parts.gfx" => Some(swf(&[import("views/a.gfx", "z")])),
                "data/common/fe/icon.dds" => Some(b"DDS ".to_vec()),
                "data/common/fe/broken.gfx" => Some(b"FWS\x08\x01".to_vec()),
                _ => None,
            }
        }
        fn exists(&self, path: &str) -> bool {
            self.read(path).is_some()
        }
        fn list(&self, _: &str, _: &str) -> Vec<String> {
            Vec::new()
        }
        fn image(&self, path: &str) -> Result<ImageData> {
            if path != "data/common/fe/icon.dds" {
                bail!("{path}: no image");
            }
            Ok(ImageData { width: 1, height: 1, rgba: vec![9; 4], source: path.into() })
        }
    }

    #[test]
    fn parses_a_view_with_its_imports_and_images_and_hands_each_out_once() {
        let keys = ["views/a.gfx", "broken.gfx"].map(String::from).to_vec();
        let ahead = MoviesAhead::start(Arc::new(Movies), keys, HashSet::new()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let icon = loop {
            if let Some(img) = ahead.take_image("data/common/fe/icon.dds") {
                break img;
            }
            assert!(std::time::Instant::now() < deadline, "icon.dds not decoded");
            std::thread::yield_now();
        };
        assert_eq!(icon.rgba, [9; 4]);
        let view = loop {
            if let Some(m) = ahead.take_movie("data/common/fe/views/a.gfx") {
                break m;
            }
            assert!(std::time::Instant::now() < deadline, "the view not parsed");
            std::thread::yield_now();
        };
        assert_eq!(view, Movie::parse(&Movies.read("data/common/fe/views/a.gfx").unwrap()).unwrap());
        let parts = loop {
            if let Some(m) = ahead.take_movie("data/common/fe/parts.gfx") {
                break m;
            }
            assert!(std::time::Instant::now() < deadline, "the imported movie not parsed");
            std::thread::yield_now();
        };
        assert_eq!(parts.frame_count, 1);
        assert!(ahead.take_movie("data/common/fe/views/a.gfx").is_none());
        assert!(ahead.take_movie("data/common/fe/parts.gfx").is_none());
        assert!(ahead.take_image("data/common/fe/icon.dds").is_none());
        assert!(ahead.take_movie("data/common/fe/broken.gfx").is_none());
        assert!(ahead.is_current(&Movies));
    }

    struct Logged(Mutex<Vec<String>>);

    impl FileSource for Logged {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            self.0.lock().unwrap().push(path.to_string());
            Movies.read(path)
        }
        fn exists(&self, path: &str) -> bool {
            Movies.exists(path)
        }
        fn list(&self, _: &str, _: &str) -> Vec<String> {
            Vec::new()
        }
        fn image(&self, path: &str) -> Result<ImageData> {
            Movies.image(path)
        }
    }

    #[test]
    fn walks_each_key_with_its_imports_before_the_next() {
        let files = Logged(Mutex::new(Vec::new()));
        let keys = ["views/a.gfx", "missing.gfx", "parts.gfx", "views/a.gfx", "broken.gfx"].map(String::from).to_vec();
        let out = Mutex::new(Found::default());
        walk(&files, keys, HashSet::new(), &out);
        let read: Vec<String> = files.0.into_inner().unwrap().into_iter().filter(|p| p.ends_with(".gfx")).collect();
        let movies = ["views/a.gfx", "parts.gfx", "missing.gfx", "broken.gfx"].map(|k| format!("{FE_ROOT}{k}"));
        let expected: Vec<String> = movies.into_iter().flat_map(|m| [m.clone(), m]).collect();
        assert_eq!(read, expected);
        let found = out.into_inner().unwrap();
        let mut parsed: Vec<&str> = found.movies.keys().map(String::as_str).collect();
        parsed.sort_unstable();
        assert_eq!(parsed, [format!("{FE_ROOT}parts.gfx"), format!("{FE_ROOT}views/a.gfx")]);
        assert_eq!(found.images.len(), 1);
    }

    #[test]
    fn skips_the_movies_the_library_holds() {
        let files = Logged(Mutex::new(Vec::new()));
        let mut held: HashSet<String> = (0..MAX_MOVIES).map(|i| format!("held{i}.gfx")).collect();
        held.insert("parts.gfx".into());
        let out = Mutex::new(Found::default());
        walk(&files, vec!["views/a.gfx".into()], held, &out);
        let view = format!("{FE_ROOT}views/a.gfx");
        let icon = format!("{FE_ROOT}icon.dds");
        assert_eq!(files.0.into_inner().unwrap(), [view.clone(), view.clone(), icon.clone()]);
        let found = out.into_inner().unwrap();
        assert_eq!(found.movies.keys().collect::<Vec<_>>(), [&view]);
        assert_eq!(found.images.keys().collect::<Vec<_>>(), [&icon]);
    }
}
