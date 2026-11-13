use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::image::ImageData;

pub const FE_ROOT: &str = "data/common/fe/";

pub trait FileSource: Send + Sync {
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    fn exists(&self, path: &str) -> bool;
    fn list(&self, dir: &str, extension: &str) -> Vec<String>;
    fn image(&self, path: &str) -> Result<ImageData>;
    fn is_content_image(&self, _path: &str) -> bool {
        false
    }
    fn content(&self) -> Option<std::sync::Arc<ssx_assets::content::GameContent>> {
        None
    }
    fn package_origin(&self, _path: &str) -> Option<PackageOrigin> {
        None
    }
    fn port_override(&self, _key: &str) -> Option<(Vec<u8>, PackageOrigin)> {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PackageOrigin {
    pub id: ssx_content::ContentId,
    pub source: String,
}

pub struct Layered {
    layers: Vec<Box<dyn FileSource>>,
}

impl Layered {
    pub fn new(layers: Vec<Box<dyn FileSource>>) -> Result<Self> {
        if layers.is_empty() {
            bail!("a layered source needs a base");
        }
        Ok(Layered { layers })
    }
}

impl FileSource for Layered {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.layers.iter().find_map(|l| l.read(path))
    }

    fn exists(&self, path: &str) -> bool {
        self.layers.iter().any(|l| l.exists(path))
    }

    fn list(&self, dir: &str, extension: &str) -> Vec<String> {
        let all: std::collections::BTreeSet<String> = self.layers.iter().flat_map(|l| l.list(dir, extension)).collect();
        all.into_iter().collect()
    }

    fn image(&self, path: &str) -> Result<ImageData> {
        let (top, base) = self.layers.split_at(self.layers.len() - 1);
        match top.iter().find(|l| l.exists(path)) {
            Some(l) => l.image(path),
            None => base[0].image(path),
        }
    }

    fn is_content_image(&self, path: &str) -> bool {
        self.layers.iter().any(|l| l.is_content_image(path))
    }

    fn content(&self) -> Option<std::sync::Arc<ssx_assets::content::GameContent>> {
        self.layers.iter().find_map(|l| l.content())
    }

    fn package_origin(&self, path: &str) -> Option<PackageOrigin> {
        self.layers.iter().find_map(|l| l.package_origin(path))
    }

    fn port_override(&self, key: &str) -> Option<(Vec<u8>, PackageOrigin)> {
        self.layers.iter().find_map(|l| l.port_override(key))
    }
}

pub struct PackageMovies {
    content: std::sync::Arc<ssx_assets::content::GameContent>,
}

impl PackageMovies {
    pub fn new(content: std::sync::Arc<ssx_assets::content::GameContent>) -> Self {
        PackageMovies { content }
    }

    fn movie(&self, path: &str, port: bool) -> Option<ssx_assets::content::m6::PackageMovie> {
        let id = ssx_content::ui::movie_id(&normalise(path), port)?;
        self.content.package_movie(&id)
    }
}

impl FileSource for PackageMovies {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.movie(path, false).map(|m| m.bytes.to_vec())
    }

    fn exists(&self, path: &str) -> bool {
        self.movie(path, false).is_some()
    }

    fn list(&self, _dir: &str, _extension: &str) -> Vec<String> {
        Vec::new()
    }

    fn image(&self, path: &str) -> Result<ImageData> {
        bail!("{path}: a package's images are textures (the disc's source resolves them by id)")
    }

    fn content(&self) -> Option<std::sync::Arc<ssx_assets::content::GameContent>> {
        Some(self.content.clone())
    }

    fn package_origin(&self, path: &str) -> Option<PackageOrigin> {
        let id = ssx_content::ui::movie_id(&normalise(path), false)?;
        self.content.package_movie(&id).map(|m| PackageOrigin { id, source: m.source })
    }

    fn port_override(&self, key: &str) -> Option<(Vec<u8>, PackageOrigin)> {
        let path = format!("{FE_ROOT}{key}");
        let id = ssx_content::ui::movie_id(&path, true)?;
        self.content.package_movie(&id).map(|m| (m.bytes.to_vec(), PackageOrigin { id, source: m.source }))
    }
}

pub struct Disc {
    archives: Vec<PathBuf>,
    root: PathBuf,
}

impl Disc {
    pub fn new(root: &Path) -> Result<Self> {
        let mut archives: Vec<PathBuf> = std::fs::read_dir(root)
            .with_context(|| format!("reading {}", root.display()))?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        archives.sort();
        Ok(Disc { archives, root: root.to_path_buf() })
    }

    pub fn find(&self, rel: &str) -> Option<PathBuf> {
        let rel = contained(rel)?;
        ssx_assets::data_roots::find(&self.root, &rel)
    }

    pub fn list(&self, dir: &str, extension: &str) -> Vec<String> {
        let Some(dir) = contained(dir) else { return Vec::new() };
        let mut out = std::collections::BTreeSet::new();
        let mut roots: Vec<PathBuf> = ssx_assets::data_roots::title_update().into_iter().collect();
        roots.extend(ssx_assets::content::dlc::active_roots());
        for root in roots {
            for prefix in [root.clone(), root.join("bootcache")] {
                walk(&prefix.join(&dir), &mut |p| {
                    if p.extension().is_some_and(|e| e.eq_ignore_ascii_case(extension))
                        && let Ok(rel) = p.strip_prefix(&prefix)
                    {
                        out.insert(rel.to_string_lossy().replace('\\', "/"));
                    }
                });
            }
        }
        for a in &self.archives {
            walk(&a.join(&dir), &mut |p| {
                if p.extension().is_some_and(|e| e.eq_ignore_ascii_case(extension))
                    && let Ok(rel) = p.strip_prefix(a)
                {
                    out.insert(rel.to_string_lossy().replace('\\', "/"));
                }
            });
        }
        out.into_iter().collect()
    }
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, f);
        } else {
            f(&p);
        }
    }
}

pub fn contained(rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    let n = normalise(rel);
    for seg in n.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s if s.contains([':', '\0']) => return None,
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

pub fn normalise(rel: &str) -> String {
    let s = rel.replace('\\', "/").to_lowercase();
    s.trim_start_matches("./").trim_start_matches('/').to_string()
}

impl FileSource for Disc {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.find(path)?).ok()
    }

    fn exists(&self, path: &str) -> bool {
        self.find(path).is_some() || self.is_content_image(path)
    }

    fn content(&self) -> Option<std::sync::Arc<ssx_assets::content::GameContent>> {
        Some(ssx_assets::content::GameContent::of(&self.root))
    }

    fn is_content_image(&self, path: &str) -> bool {
        ssx_assets::content::textures::ui_image_id(&normalise(path))
            .is_some_and(|id| ssx_assets::content::GameContent::of(&self.root).has(&id))
    }

    fn list(&self, dir: &str, extension: &str) -> Vec<String> {
        Disc::list(self, dir, extension)
    }

    fn image(&self, path: &str) -> Result<ImageData> {
        let full = normalise(path);
        if !(full.ends_with(".xpr") || full.ends_with(".dds")) {
            bail!("{path}: no reader for this image type");
        }
        if let Some(file) = ssx_assets::data_roots::overlay_only(&full) {
            let texture = ssx_assets::content::textures::decode_path(&file)
                .with_context(|| format!("decoding {} ({path})", file.display()))?;
            let level = texture.levels.into_iter().next().with_context(|| format!("{path}: no level"))?;
            return Ok(ImageData {
                width: level.width,
                height: level.height,
                rgba: level.images.into_iter().next().with_context(|| format!("{path}: no image"))?,
                source: path.to_string(),
            });
        }
        let id = ssx_assets::content::textures::ui_image_id(&full)
            .or_else(|| ssx_assets::content::textures::common_texture_id(&full))
            .with_context(|| format!("{path}: not a texture id"))?;
        let found = ssx_assets::content::GameContent::of(&self.root)
            .texture(&id, &ssx_content::texture::Want::PLANE)
            .with_context(|| format!("decoding {path}"))?;
        let level = found.texture.levels.into_iter().next().with_context(|| format!("{path}: no level"))?;
        let (width, height) = (level.width, level.height);
        let rgba = level.images.into_iter().next().with_context(|| format!("{path}: no image"))?;
        Ok(ImageData { width, height, rgba, source: path.to_string() })
    }
}

pub fn join(dir: &str, url: &str) -> String {
    let mut parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    let url = url.replace('\\', "/");
    for seg in url.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_relative_urls() {
        assert_eq!(join("views/hud", "../components/HUD/x.swf"), "views/components/HUD/x.swf");
        assert_eq!(join("", "components/a.swf"), "components/a.swf");
        assert_eq!(normalise("Data\\Common\\FE\\x.GFX"), "data/common/fe/x.gfx");
    }

    #[test]
    fn paths_stay_inside_their_root() {
        assert_eq!(
            contained("data/common/fe/views/../components/a.gfx").as_deref(),
            Some("data/common/fe/components/a.gfx")
        );
        assert_eq!(contained("./data//x.gfx").as_deref(), Some("data/x.gfx"));
        for bad in ["../../etc/passwd", "data/../../x", "c:/windows/x", "data/a:b", "data/x\0y", "/../x"] {
            assert_eq!(contained(bad), None, "{bad}");
        }
    }
}
