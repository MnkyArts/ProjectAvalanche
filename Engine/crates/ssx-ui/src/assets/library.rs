use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use ssx_formats::gfx::avm1::Action;
use ssx_formats::gfx::place::PlaceObject;
use ssx_formats::gfx::shape::{DefineShape, MorphShape};
use ssx_formats::gfx::text::{EditText, Font, StaticText};
use ssx_formats::gfx::types::Rgba;
use ssx_formats::gfx::{Movie, Tag, TagRecord};

use super::ahead::{self, Ahead, MoviesAhead};
use crate::fs::{FE_ROOT, FileSource, PackageOrigin, normalise};
use crate::geom::RectF;
use crate::image::{self, ImageData};

pub type MovieId = usize;
pub type ImageId = usize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CharRef {
    pub movie: MovieId,
    pub id: u16,
}

#[derive(Clone, Debug)]
pub enum FrameTag {
    Place(Arc<PlaceObject>),
    Remove(u16),
}

#[derive(Clone, Debug, Default)]
pub struct FrameDef {
    pub tags: Vec<FrameTag>,
    pub actions: Vec<Arc<[Action]>>,
    pub init_actions: Vec<(u16, Arc<[Action]>)>,
    pub labels: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct TimelineDef {
    pub movie: MovieId,
    pub id: u16,
    pub frames: Vec<FrameDef>,
}

impl TimelineDef {
    pub fn frame_count(&self) -> u16 {
        self.frames.len() as u16
    }

    pub fn label_frame(&self, label: &str) -> Option<u16> {
        self.frames.iter().position(|f| f.labels.iter().any(|l| l.eq_ignore_ascii_case(label))).map(|i| i as u16 + 1)
    }

    fn build(movie: MovieId, id: u16, frame_count: u16, tags: &[TagRecord]) -> TimelineDef {
        let mut frames = Vec::with_capacity(usize::from(frame_count));
        let mut cur = FrameDef::default();
        for t in tags {
            match &t.tag {
                Tag::ShowFrame => frames.push(std::mem::take(&mut cur)),
                Tag::PlaceObject(p) => cur.tags.push(FrameTag::Place(Arc::new((**p).clone()))),
                Tag::RemoveObject2 { depth } => cur.tags.push(FrameTag::Remove(*depth)),
                Tag::DoAction(a) => cur.actions.push(Arc::from(a.as_slice())),
                Tag::DoInitAction { sprite_id, actions } => {
                    cur.init_actions.push((*sprite_id, Arc::from(actions.as_slice())))
                }
                Tag::FrameLabel { name, .. } => cur.labels.push(name.clone()),
                _ => {}
            }
        }
        let n = usize::from(frame_count.max(1));
        frames.resize_with(n, FrameDef::default);
        frames.truncate(n);
        TimelineDef { movie, id, frames }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BitmapDef {
    pub image: ImageId,
    pub rect: [u32; 4],
    pub size: [u32; 2],
}

impl BitmapDef {
    pub fn of_rect(image: ImageId, rect: [u32; 4]) -> Self {
        BitmapDef { image, rect, size: [rect[2] - rect[0], rect[3] - rect[1]] }
    }

    pub fn width(&self) -> u32 {
        self.rect[2] - self.rect[0]
    }

    pub fn height(&self) -> u32 {
        self.rect[3] - self.rect[1]
    }

    pub fn texel_to_pixel(&self) -> crate::geom::Mat2d {
        let k = |size: u32, texels: u32| if texels == 0 { 1.0 } else { size as f32 / texels as f32 };
        crate::geom::Mat2d::scale(k(self.size[0], self.width()), k(self.size[1], self.height()))
    }
}

#[derive(Clone, Debug)]
pub enum Character {
    Shape(Arc<DefineShape>),
    Morph(Arc<MorphShape>),
    Sprite(Arc<TimelineDef>),
    EditText(Arc<EditText>),
    Text(Arc<StaticText>),
    Bitmap(BitmapDef),
    Font(Arc<Font>),
}

impl Character {
    pub fn kind(&self) -> &'static str {
        match self {
            Character::Shape(_) => "shape",
            Character::Morph(_) => "morph shape",
            Character::Sprite(_) => "sprite",
            Character::EditText(_) => "edit text",
            Character::Text(_) => "static text",
            Character::Bitmap(_) => "bitmap",
            Character::Font(_) => "font",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Import {
    pub url: String,
    pub name: String,
    pub target: Option<CharRef>,
}

#[derive(Clone, Debug)]
pub struct MovieDef {
    pub path: String,
    pub dir: String,
    pub version: u8,
    pub frame_rate: f32,
    pub stage: RectF,
    pub background: Option<Rgba>,
    pub swf_name: String,
    pub root: Arc<TimelineDef>,
    pub chars: HashMap<u16, Character>,
    pub exports: HashMap<String, u16>,
    pub imports: HashMap<u16, Import>,
    pub scaling_grids: HashMap<u16, RectF>,
}

pub type MovieBuilder = Arc<dyn Fn(&mut Library) -> Result<MovieId> + Send + Sync>;

pub struct Library {
    fs: Arc<dyn FileSource>,
    ahead: Option<Ahead>,
    movies_ahead: Option<MoviesAhead>,
    movies: Vec<MovieDef>,
    by_path: HashMap<String, MovieId>,
    builders: HashMap<String, MovieBuilder>,
    images: Vec<Arc<ImageData>>,
    image_by_path: HashMap<String, ImageId>,
    font_lib: HashMap<String, Arc<Font>>,
    pub log: Vec<String>,
    package_movies: HashMap<MovieId, PackageOrigin>,
    fallback: Vec<Arc<Font>>,
    disc_fallback: Vec<Arc<Font>>,
    disc_glyphs: HashMap<u16, (usize, usize)>,
    package_ot: Vec<Arc<crate::ot::OtFont>>,
    text_budget: crate::ot::Budget,
    import_depth: usize,
}

pub const MAX_IMPORT_DEPTH: usize = 16;
pub const MAX_MOVIES: usize = 8192;

pub fn movie_key(rel: &str) -> String {
    let mut k = normalise(rel);
    if let Some(stem) = k.strip_suffix(".swf") {
        k = format!("{stem}.gfx");
    }
    if let Some(rest) = k.strip_prefix(FE_ROOT) {
        k = rest.to_string();
    }
    k
}

pub(crate) fn first_glyphs(fonts: &[Arc<Font>]) -> HashMap<u16, (usize, usize)> {
    let mut out = HashMap::new();
    for (i, f) in fonts.iter().enumerate() {
        for &code in &f.codes {
            let Some(c) = char::from_u32(u32::from(code)) else { continue };
            if !out.contains_key(&code)
                && let Some(glyph) = crate::text::glyph_index(f, c)
            {
                out.insert(code, (i, glyph));
            }
        }
    }
    out
}

impl Library {
    pub fn new(fs: Box<dyn FileSource>) -> Self {
        Library {
            fs: Arc::from(fs),
            ahead: None,
            movies_ahead: None,
            movies: Vec::new(),
            by_path: HashMap::new(),
            builders: HashMap::new(),
            images: Vec::new(),
            image_by_path: HashMap::new(),
            font_lib: HashMap::new(),
            log: Vec::new(),
            package_movies: HashMap::new(),
            fallback: Vec::new(),
            disc_fallback: Vec::new(),
            disc_glyphs: HashMap::new(),
            package_ot: Vec::new(),
            text_budget: crate::ot::Budget::default(),
            import_depth: 0,
        }
    }

    pub fn origin(&self, movie: MovieId) -> Option<&PackageOrigin> {
        self.package_movies.get(&movie)
    }

    pub fn has_package_movies(&self) -> bool {
        !self.package_movies.is_empty()
    }

    pub fn forget_movies(&mut self, changed: impl Fn(&str) -> bool) -> usize {
        self.ahead = None;
        let keys: Vec<String> = self.by_path.keys().filter(|k| changed(&format!("{FE_ROOT}{k}"))).cloned().collect();
        for k in &keys {
            self.by_path.remove(k);
        }
        keys.len()
    }

    pub fn fallback_fonts(&self) -> &[Arc<Font>] {
        &self.fallback
    }

    pub fn disc_fallback(&self) -> &[Arc<Font>] {
        &self.disc_fallback
    }

    pub fn disc_glyph(&self, c: char) -> Option<(&Arc<Font>, usize)> {
        let code = u16::try_from(u32::from(c)).ok()?;
        let &(font, glyph) = self.disc_glyphs.get(&code)?;
        Some((&self.disc_fallback[font], glyph))
    }

    pub fn package_ot(&self) -> &[Arc<crate::ot::OtFont>] {
        &self.package_ot
    }

    pub fn text_budget(&self) -> &crate::ot::Budget {
        &self.text_budget
    }

    pub fn files(&self) -> &dyn FileSource {
        self.fs.as_ref()
    }

    pub fn movie(&self, id: MovieId) -> &MovieDef {
        &self.movies[id]
    }

    pub fn movies(&self) -> &[MovieDef] {
        &self.movies
    }

    pub fn image(&self, id: ImageId) -> &Arc<ImageData> {
        &self.images[id]
    }

    pub fn images(&self) -> &[Arc<ImageData>] {
        &self.images
    }

    pub fn movie_by_path(&self, rel: &str) -> Option<MovieId> {
        self.by_path.get(&movie_key(rel)).copied()
    }

    pub fn insert_movie(&mut self, m: MovieDef) -> MovieId {
        let id = self.movies.len();
        self.by_path.insert(m.path.clone(), id);
        self.movies.push(m);
        id
    }

    pub fn insert_image(&mut self, img: ImageData) -> ImageId {
        self.images.push(Arc::new(img));
        self.images.len() - 1
    }

    pub fn reload_images(&mut self, changed: impl Fn(&str) -> bool) -> usize {
        let mut n = 0;
        for i in 0..self.images.len() {
            let source = self.images[i].source.clone();
            if !changed(&source) {
                continue;
            }
            match image::load(self.fs.as_ref(), &source) {
                Ok(img) => {
                    self.images[i] = Arc::new(img);
                    n += 1;
                }
                Err(e) => self.log.push(format!("reloading {source}: {e:#}")),
            }
        }
        n
    }

    pub fn load_font_library(&mut self, region: &str) -> Result<MovieId> {
        let id = self.load_movie(&format!("gfxfontlib/{region}/gfxfontlib.gfx"))?;
        let mut fonts: Vec<Arc<Font>> = self.movies[id]
            .chars
            .values()
            .filter_map(|c| match c {
                Character::Font(f) if !f.glyphs.is_empty() => Some(f.clone()),
                _ => None,
            })
            .collect();
        fonts.sort_by_key(|f| f.id);
        self.disc_fallback = fonts.clone();
        for f in fonts {
            self.font_lib.insert(f.name.clone(), f);
        }
        let other = if region == "jp" { "na" } else { "jp" };
        let file = format!("{FE_ROOT}gfxfontlib/{other}/gfxfontlib.gfx");
        match self.fs.read(&file).map(|d| Movie::parse(&d)) {
            Some(Ok(m)) => self.disc_fallback.extend(m.tags.iter().filter_map(|t| match &t.tag {
                Tag::Font(f) if !f.glyphs.is_empty() => Some(Arc::new(f.clone())),
                _ => None,
            })),
            Some(Err(e)) => self.log.push(format!("{file}: the other region's fonts not read: {e}")),
            None => self.log.push(format!("{file}: not found (the other region's fonts)")),
        }
        self.disc_glyphs = first_glyphs(&self.disc_fallback);
        self.apply_package_fonts(region);
        Ok(id)
    }

    pub fn apply_package_fonts(&mut self, region: &str) {
        let Some(content) = self.fs.content() else { return };
        self.fallback.clear();
        self.package_ot.clear();
        for pf in content.package_fonts() {
            let mut font = pf.font.font.clone();
            match &pf.replaces {
                Some(lib) if lib.region == region => font.name = lib.name.clone(),
                Some(_) => continue,
                None => font.name = pf.id.as_str().to_string(),
            }
            let font = Arc::new(font);
            self.font_lib.insert(font.name.clone(), font.clone());
            self.fallback.push(font);
            let bytes = ssx_content::font::lazy::Bytes(pf.font.bytes.clone());
            match crate::ot::OtFont::open(pf.id.as_str(), bytes, &ssx_content::font::Limits::default()) {
                Ok(f) => self.package_ot.push(Arc::new(f)),
                Err(e) => self.log.push(format!("font {}: not shaped: {e}", pf.id.as_str())),
            }
        }
    }

    pub fn font_library(&self) -> &HashMap<String, Arc<Font>> {
        &self.font_lib
    }

    // TODO(guess): a zero-glyph font resolves by name to the font library's export (the disc's placeholders match by name); settle: Scaleform's FontLib / FontMap lookup (font library set up at 0x82594A68).
    pub fn font_for(&self, owner: MovieId, font_id: Option<u16>, face: Option<&str>) -> Option<Arc<Font>> {
        let own = font_id.and_then(|i| match self.resolve(CharRef { movie: owner, id: i }) {
            Some((_, Character::Font(f))) => Some(f.clone()),
            _ => None,
        });
        if let Some(f) = own.as_ref().filter(|f| !f.glyphs.is_empty() && face.is_none_or(|n| n == f.name)) {
            return Some(f.clone());
        }
        let name = face.map(str::to_string).or_else(|| own.as_ref().map(|f| f.name.clone()))?;
        self.font_lib.get(&name).cloned()
    }

    pub fn resolve(&self, r: CharRef) -> Option<(CharRef, &Character)> {
        let mut r = r;
        for _ in 0..16 {
            let m = self.movies.get(r.movie)?;
            if let Some(c) = m.chars.get(&r.id) {
                return Some((r, c));
            }
            r = m.imports.get(&r.id)?.target?;
        }
        None
    }

    pub fn export(&self, movie: MovieId, name: &str) -> Option<CharRef> {
        let m = self.movies.get(movie)?;
        let id = m
            .exports
            .get(name)
            .or_else(|| m.exports.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|e| e.1))?;
        self.resolve(CharRef { movie, id: *id }).map(|(r, _)| r)
    }

    // TODO(guess): the root's definition list (+0x3C) is walked newest first (only that order lets the main menu's `ListMenu` find the `CellWrapper`, as the console does); settle: the list's order.
    pub fn export_from_importers(
        &self,
        movie: MovieId,
        name: &str,
        loaded: impl Fn(MovieId) -> bool,
    ) -> Option<CharRef> {
        let mut current = movie;
        for (i, m) in self.movies.iter().enumerate().rev() {
            if i == movie || !loaded(i) {
                continue;
            }
            if m.imports.values().any(|imp| imp.target.is_some_and(|t| t.movie == current)) {
                if let Some(r) = self.export(i, name) {
                    return Some(r);
                }
                current = i;
            }
        }
        None
    }

    pub fn scaling_grid(&self, r: CharRef) -> Option<RectF> {
        self.movies.get(r.movie)?.scaling_grids.get(&r.id).copied()
    }

    fn image_file(&mut self, path: &str) -> Option<ImageId> {
        if let Some(&id) = self.image_by_path.get(path) {
            return Some(id);
        }
        let loaded = match self.take_movie_image_ahead(path) {
            Some(img) => Ok(img),
            None => image::load(self.fs.as_ref(), path),
        };
        match loaded {
            Ok(img) => {
                let id = self.images.len();
                self.images.push(Arc::new(img));
                self.image_by_path.insert(path.to_string(), id);
                Some(id)
            }
            Err(e) => {
                self.log.push(format!("{e:#}"));
                None
            }
        }
    }

    fn load_image_movie(&mut self, key: &str, file: &str) -> Result<MovieId> {
        let img = match self.take_movie_image_ahead(file) {
            Some(img) => img,
            None => image::load(self.fs.as_ref(), file).with_context(|| format!("loading {file} as an image"))?,
        };
        self.image_movie(key, file, img)
    }

    pub fn add_image_movie(&mut self, rel: &str, img: ImageData) -> Result<MovieId> {
        let key = movie_key(rel);
        if self.builders.contains_key(&key) || self.fs.exists(&format!("{FE_ROOT}{key}")) {
            anyhow::bail!("{key}: a movie of that path exists");
        }
        if let Some(&id) = self.by_path.get(&key) {
            return Ok(id);
        }
        if self.movies.len() >= MAX_MOVIES {
            anyhow::bail!("{key}: the library holds {MAX_MOVIES} movies already");
        }
        if img.rgba.len() as u64 != u64::from(img.width) * u64::from(img.height) * 4 {
            anyhow::bail!("{key}: {} bytes of RGBA for {} x {} pixels", img.rgba.len(), img.width, img.height);
        }
        self.image_movie(&key, &key, img)
    }

    // TODO(guess): the image movie's layout (one frame, the bitmap at depth 1 at 0, 0, the stage the image's size, 30 fps) follows Flash's loadMovie of a bitmap; settle: Scaleform's load-queue image branch.
    fn image_movie(&mut self, key: &str, file: &str, img: ImageData) -> Result<MovieId> {
        let (w, h) = (img.width, img.height);
        let image = self.insert_image(img);
        let id = self.movies.len();
        let dir = file.rsplit_once('/').map_or(String::new(), |(d, _)| format!("{d}/"));
        let place = PlaceObject {
            version: 2,
            flags: PlaceObject::HAS_CHARACTER | 0x04,
            flags3: 0,
            depth: 1,
            class_name: None,
            character_id: Some(1),
            matrix: Some(ssx_formats::gfx::types::Matrix::IDENTITY),
            color_transform: None,
            ratio: None,
            name: None,
            clip_depth: None,
            filters: Vec::new(),
            blend_mode: None,
            cache_as_bitmap: None,
            visible: None,
            background: None,
            clip_actions: None,
        };
        let frame = FrameDef { tags: vec![FrameTag::Place(Arc::new(place))], ..Default::default() };
        let mut chars = HashMap::new();
        chars.insert(1, Character::Bitmap(BitmapDef::of_rect(image, [0, 0, w, h])));
        self.movies.push(MovieDef {
            path: key.to_string(),
            dir,
            version: 8,
            frame_rate: 30.0,
            stage: RectF::new(0.0, 0.0, w as f32 * 20.0, h as f32 * 20.0),
            background: None,
            swf_name: String::new(),
            root: Arc::new(TimelineDef { movie: id, id: 0, frames: vec![frame] }),
            chars,
            exports: HashMap::new(),
            imports: HashMap::new(),
            scaling_grids: HashMap::new(),
        });
        self.by_path.insert(key.to_string(), id);
        Ok(id)
    }

    pub fn add_builder(&mut self, rel: &str, builder: MovieBuilder) -> Result<()> {
        let key = movie_key(rel);
        if self.by_path.contains_key(&key)
            || self.builders.contains_key(&key)
            || self.fs.exists(&format!("{FE_ROOT}{key}"))
        {
            anyhow::bail!("the movie {key} exists already");
        }
        self.builders.insert(key, builder);
        Ok(())
    }

    pub fn load_movie(&mut self, rel: &str) -> Result<MovieId> {
        let key = movie_key(rel);
        if let Some(&id) = self.by_path.get(&key) {
            return Ok(id);
        }
        if self.movies.len() >= MAX_MOVIES {
            anyhow::bail!("{key}: the library holds {MAX_MOVIES} movies already");
        }
        if self.import_depth >= MAX_IMPORT_DEPTH {
            anyhow::bail!("{key}: imports nested deeper than {MAX_IMPORT_DEPTH}");
        }
        if self.builders.contains_key(&key)
            && let Some((data, origin)) = self.fs.port_override(&key)
        {
            return self.parse_movie(&key, &format!("{FE_ROOT}{key}"), &data, Some(origin));
        }
        if let Some(build) = self.builders.get(&key).cloned() {
            let id = build(self).with_context(|| format!("building {key}"))?;
            if self.movies.get(id).is_none_or(|m| m.path != key) {
                anyhow::bail!("the builder of {key} added no movie of that path");
            }
            return Ok(id);
        }
        let file = format!("{FE_ROOT}{key}");
        if let Some(img) = self.take_ahead(&file) {
            return self.image_movie(&key, &file, img);
        }
        if self.fs.is_content_image(&file) {
            return self.load_image_movie(&key, &file);
        }
        if let Some(parsed) = self.take_movie_ahead(&file) {
            let origin = self.fs.package_origin(&file);
            return self.add_movie(&key, &file, parsed, origin);
        }
        let data = self.fs.read(&file).with_context(|| format!("movie {file} not found"))?;
        if !ahead::is_movie(&data) {
            return self.load_image_movie(&key, &file);
        }
        let origin = self.fs.package_origin(&file);
        self.parse_movie(&key, &file, &data, origin)
    }

    pub fn parse_ahead(&mut self, rels: impl IntoIterator<Item = String>) {
        let keys: Vec<String> = rels
            .into_iter()
            .map(|r| movie_key(&r))
            .filter(|k| !self.by_path.contains_key(k) && !self.builders.contains_key(k))
            .collect();
        if keys.is_empty() {
            return;
        }
        // PORT(choice): the movies a view will load are parsed (with the movies they import and their external images
        // decoded) beforehand on a thread, and the load takes the parsed movie and the decoded images instead of
        // reading, parsing and decoding them on the main thread. The shell asks when a level's load starts for the
        // HUD's view `views/hud.gfx`, and in the frame the level's loaded data arrives (before its spawn), once the
        // view is in the library, for the views
        // of the HUD's containers as the framework's ViewMap names them (`GameUi::hud_view_files`: seven
        // `views/hud/*container.swf` on the disc); the movies the library holds are not parsed again. The view
        // parsed in the level's spawn frame took 24-57 ms of it; a container the HUD's navigation loads in the
        // ride's first second (dynamicleaderboard, boostnotification, hudmeters, 0.07-0.21 s in) took 33-40 ms of its
        // frame with its files out of the system's file cache, 26-35 ms of it reading the container's file, and 2-5 ms
        // with them in it (alps0101; development laptop, `trace_chrome`). The movie and the images are the ones the
        // load reads (the same file source and decision; a change of the mounted packages, the title update or the
        // content registry discards them), and the library adds them in the load's order, in the update the load comes
        // in; a movie not parsed yet is loaded as before. The console's resource loads are not decoded.
        let held = self.by_path.keys().chain(self.builders.keys()).cloned().collect();
        match MoviesAhead::start(self.fs.clone(), keys, held) {
            Ok(a) => self.movies_ahead = Some(a),
            Err(e) => {
                self.movies_ahead = None;
                self.log.push(format!("movies ahead: no thread ({e}); each movie is parsed at its load"));
            }
        }
    }

    fn take_movie_ahead(&mut self, file: &str) -> Option<Movie> {
        let ahead = self.movies_ahead.as_ref()?;
        if !ahead.is_current(self.fs.as_ref()) {
            self.movies_ahead = None;
            return None;
        }
        ahead.take_movie(file)
    }

    fn take_movie_image_ahead(&mut self, path: &str) -> Option<ImageData> {
        let ahead = self.movies_ahead.as_ref()?;
        if !ahead.is_current(self.fs.as_ref()) {
            self.movies_ahead = None;
            return None;
        }
        ahead.take_image(path)
    }

    pub fn decode_ahead(&mut self, paths: impl IntoIterator<Item = String>) {
        let mut seen = std::collections::HashSet::new();
        let files: Vec<String> = paths
            .into_iter()
            .map(|p| movie_key(&p))
            .filter(|k| !self.by_path.contains_key(k) && !self.builders.contains_key(k) && seen.insert(k.clone()))
            .take(ahead::MAX_FILES)
            .map(|k| format!("{FE_ROOT}{k}"))
            .collect();
        // PORT(choice): images a movie will load are decoded beforehand on a thread, and the load takes the decoded
        // image instead of reading and decoding the file on the main thread. The game asks for the notifications' icons
        // at its start: the 177 badges' and the geotag's (`assets/notificationIcons/*.dds`, 64 x 64, 0.75 MB of files,
        // 2.92 MB decoded; 84 ms on the thread with the files out of the cache), each loaded by `hud.vm`'s
        // `mcInGameNotification` in the frame its notification shows. A file not in the system's file cache made that
        // frame wait on the disk: the load 6.1-9.3 ms (0.7-1.3 ms of it on the CPU, no page fault), the frame's main
        // thread up to 62 ms, on the development laptop (Mount Eddie's badges, GitHub #95). The image is the one the
        // load decodes (the same file source and decision; a change of the mounted packages, the title update or the
        // content registry discards the decoded images) and shows in the same frame; an image not decoded yet is loaded
        // as before. The console's image creator (0x825A4050) opened the file at the load, on its own file system.
        match Ahead::start(self.fs.clone(), files) {
            Ok(a) => self.ahead = Some(a),
            Err(e) => {
                self.ahead = None;
                self.log.push(format!("images ahead: no thread ({e}); each image is read at its load"));
            }
        }
    }

    fn take_ahead(&mut self, file: &str) -> Option<ImageData> {
        let ahead = self.ahead.as_ref()?;
        if !ahead.is_current(self.fs.as_ref()) {
            self.ahead = None;
            return None;
        }
        ahead.take(file)
    }

    fn parse_movie(&mut self, key: &str, file: &str, data: &[u8], origin: Option<PackageOrigin>) -> Result<MovieId> {
        let parsed = Movie::parse(data).with_context(|| format!("parsing {file}"))?;
        self.add_movie(key, file, parsed, origin)
    }

    fn add_movie(&mut self, key: &str, file: &str, parsed: Movie, origin: Option<PackageOrigin>) -> Result<MovieId> {
        let key = key.to_string();
        let id = self.movies.len();
        let dir = file.rsplit_once('/').map_or(String::new(), |(d, _)| format!("{d}/"));
        let mut m = MovieDef {
            path: key.clone(),
            dir: dir.clone(),
            version: parsed.version,
            frame_rate: f32::from(parsed.frame_rate) / 256.0,
            stage: RectF::from_swf(&parsed.frame_size),
            background: None,
            swf_name: parsed.exporter_info().map(|e| e.swf_name.clone()).unwrap_or_default(),
            root: Arc::new(TimelineDef::build(id, 0, parsed.frame_count, &parsed.tags)),
            chars: HashMap::new(),
            exports: HashMap::new(),
            imports: HashMap::new(),
            scaling_grids: HashMap::new(),
        };
        let mut atlases: HashMap<u32, ImageId> = HashMap::new();
        for t in &parsed.tags {
            match &t.tag {
                Tag::Shape(s) => drop(m.chars.insert(s.id, Character::Shape(Arc::new(s.clone())))),
                Tag::MorphShape(s) => drop(m.chars.insert(s.id, Character::Morph(Arc::new(s.clone())))),
                Tag::Sprite(s) => {
                    let tl = TimelineDef::build(id, s.id, s.frame_count, &s.tags);
                    m.chars.insert(s.id, Character::Sprite(Arc::new(tl)));
                }
                Tag::EditText(e) => drop(m.chars.insert(e.id, Character::EditText(Arc::new(e.clone())))),
                Tag::Text(s) => drop(m.chars.insert(s.id, Character::Text(Arc::new(s.clone())))),
                Tag::Font(f) => drop(m.chars.insert(f.id, Character::Font(Arc::new(f.clone())))),
                Tag::SetBackgroundColor(c) => m.background = Some(*c),
                Tag::ExportAssets(a) => {
                    for (cid, name) in a {
                        m.exports.entry(name.clone()).or_insert(*cid);
                    }
                }
                Tag::ImportAssets { url, assets, .. } => {
                    for (cid, name) in assets {
                        m.imports.insert(*cid, Import { url: url.clone(), name: name.clone(), target: None });
                    }
                }
                Tag::ScalingGrid { id: cid, splitter } => drop(m.scaling_grids.insert(*cid, RectF::from_swf(splitter))),
                Tag::ExternalImage(e) => {
                    let Some(path) = image::resolve_external(self.fs.as_ref(), &dir, &e.file_name) else {
                        self.log.push(format!("{key}: image {} not found", e.file_name));
                        continue;
                    };
                    let Some(img) = self.image_file(&path) else { continue };
                    let (w, h) = (self.images[img].width, self.images[img].height);
                    if (w, h) != (u32::from(e.target_width), u32::from(e.target_height)) {
                        self.log.push(format!(
                            "{key}: image {} is {w}×{h}, the tag says {}×{}",
                            e.file_name, e.target_width, e.target_height
                        ));
                    }
                    if e.id & ssx_formats::gfx::scaleform::ExternalImage::EXPORTER_IMAGE != 0 {
                        atlases.insert(e.id, img);
                    } else {
                        let size = [u32::from(e.target_width), u32::from(e.target_height)];
                        m.chars
                            .insert(e.id as u16, Character::Bitmap(BitmapDef { image: img, rect: [0, 0, w, h], size }));
                    }
                }
                Tag::SubImage(s) => {
                    let atlas = ssx_formats::gfx::scaleform::ExternalImage::EXPORTER_IMAGE | u32::from(s.image);
                    match atlases.get(&atlas) {
                        Some(&img) => {
                            let rect = [s.x1, s.y1, s.x2, s.y2].map(u32::from);
                            m.chars.insert(s.id, Character::Bitmap(BitmapDef::of_rect(img, rect)));
                        }
                        None => self.log.push(format!("{key}: sub image {} of a missing atlas", s.id)),
                    }
                }
                _ => {}
            }
        }
        self.movies.push(m);
        self.by_path.insert(key.clone(), id);
        if let Some(o) = origin {
            self.package_movies.insert(id, o);
        }
        let mut imports: Vec<(u16, String, String)> =
            self.movies[id].imports.iter().map(|(cid, i)| (*cid, i.url.clone(), i.name.clone())).collect();
        imports.sort();
        for (cid, url, name) in imports {
            self.import_depth += 1;
            let loaded = self.load_movie(&url);
            self.import_depth -= 1;
            let target = match loaded {
                Ok(other) => match self.movies[other].exports.get(&name) {
                    Some(&oid) => Some(CharRef { movie: other, id: oid }),
                    None => {
                        self.log.push(format!("{key}: {url} does not export {name}"));
                        None
                    }
                },
                Err(e) => {
                    self.log.push(format!("{key}: import {name} from {url}: {e:#}"));
                    None
                }
            };
            if let Some(i) = self.movies[id].imports.get_mut(&cid) {
                i.target = target;
            }
        }
        Ok(id)
    }
}
