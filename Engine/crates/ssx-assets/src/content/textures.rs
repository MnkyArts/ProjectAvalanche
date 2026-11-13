use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use ssx_content::texture::{BlockChain, BlockFormat, BlockLevel, Level, Shape, Texture};
use ssx_content::{Asset, BASE_NAMESPACE, ContentId, Kind, LoadError, Source};
use ssx_formats::dds::{DdsFile, DdsFormat};
use ssx_formats::xenos::TextureFormat;
use ssx_formats::xpr::{Swizzle, XprFile};

use super::vaults::rel;
use crate::texture::{self, DxnExport};

pub const GAME_EXPLORER: &str = "data/xenon/gameexplorer";
pub const COMMON: &str = "data/common";

pub fn texture_id(rel: &str) -> Option<ContentId> {
    let p = rel.replace('\\', "/");
    let p = p.strip_suffix(".xpr").unwrap_or(&p);
    ContentId::new(BASE_NAMESPACE, Kind::Texture, &super::path_segments(p)?).ok()
}

pub fn common_texture_id(archive_path: &str) -> Option<ContentId> {
    let p = archive_path.replace('\\', "/");
    let below = p.strip_prefix(&format!("{COMMON}/"))?;
    let stem = below.rsplit_once('.').map_or(below, |(s, _)| s);
    ContentId::new(BASE_NAMESPACE, Kind::Texture, &super::path_segments(stem)?).ok()
}

// PORT(choice): `mods`, a folder no disc file lies in, below the front end's root; the console loads no packages.
pub const UI_PACKAGE_DIR: &str = "mods";

pub fn ui_image_path(id: &ContentId) -> Option<String> {
    let ns = id.namespace();
    (id.kind() == Kind::Texture && ns != BASE_NAMESPACE && ns != ssx_content::PORT_NAMESPACE)
        .then(|| format!("{UI_PACKAGE_DIR}/{ns}/{}.dds", id.path()))
}

pub fn ui_path_of(id: &ContentId) -> Option<String> {
    if id.kind() != Kind::Texture {
        return None;
    }
    if id.namespace() == BASE_NAMESPACE {
        return id.path().strip_prefix("fe/").map(|p| format!("{p}.dds"));
    }
    ui_image_path(id)
}

pub fn fe_texture_id(fe_path: &str) -> Option<ContentId> {
    common_texture_id(&format!("{COMMON}/fe/{}", fe_path.replace('\\', "/").to_ascii_lowercase()))
}

pub fn ui_image_id(archive_path: &str) -> Option<ContentId> {
    let p = archive_path.replace('\\', "/").to_ascii_lowercase();
    let below = p.strip_prefix(&format!("{COMMON}/fe/{UI_PACKAGE_DIR}/"))?.strip_suffix(".dds")?;
    let (ns, path) = below.split_once('/')?;
    if ns == BASE_NAMESPACE || ns == ssx_content::PORT_NAMESPACE {
        return None;
    }
    ContentId::new(ns, Kind::Texture, path).ok()
}

const MAX_DEPTH: usize = 32;

pub(super) fn game_archives(big_root: &Path) -> Result<Vec<PathBuf>> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(big_root)
        .with_context(|| format!("listing {}", big_root.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    out.sort();
    Ok(out)
}

pub(super) fn game_files(root: &Path, ext: &str) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !root.is_dir() {
        return Ok(out);
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut dirs = vec![(root.to_path_buf(), 0usize)];
    while let Some((d, depth)) = dirs.pop() {
        let canonical = std::fs::canonicalize(&d).with_context(|| format!("resolving {}", d.display()))?;
        if depth > MAX_DEPTH || !seen.insert(canonical) {
            continue;
        }
        for e in std::fs::read_dir(&d).with_context(|| format!("listing {}", d.display()))? {
            let p = e.with_context(|| format!("listing {}", d.display()))?.path();
            if p.is_dir() {
                dirs.push((p, depth + 1));
            } else if p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)) {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

pub struct DiscTextures {
    files: BTreeMap<ContentId, PathBuf>,
    pub collisions: Vec<(ContentId, Vec<PathBuf>)>,
}

impl DiscTextures {
    pub fn index(big_root: &Path) -> Result<Self> {
        Self::index_roots(game_archives(big_root)?)
    }

    pub fn index_archive(root: &Path) -> Result<Self> {
        Self::index_roots(vec![root.to_path_buf()])
    }

    fn index_roots(roots: Vec<PathBuf>) -> Result<Self> {
        let mut found: BTreeMap<ContentId, BTreeMap<String, PathBuf>> = BTreeMap::new();
        let mut common: BTreeMap<ContentId, [Option<(String, PathBuf)>; 2]> = BTreeMap::new();
        for a in roots {
            let ge = a.join(GAME_EXPLORER);
            for f in game_files(&ge, "xpr")? {
                let below = rel(&ge, &f);
                if let Some(id) = texture_id(&below) {
                    found.entry(id).or_default().entry(format!("{GAME_EXPLORER}/{below}")).or_insert(f);
                }
            }
            let cm = a.join(COMMON);
            for (slot, ext) in [(0, "xpr"), (1, "dds")] {
                for f in game_files(&cm, ext)? {
                    let below = format!("{COMMON}/{}", rel(&cm, &f));
                    let Some(id) = common_texture_id(&below) else { continue };
                    common.entry(id).or_default()[slot].get_or_insert((below, f));
                }
            }
        }
        for (id, [xpr, dds]) in common {
            if let Some((below, f)) = xpr.or(dds) {
                found.entry(id).or_default().entry(below).or_insert(f);
            }
        }
        let mut files = BTreeMap::new();
        let mut collisions = Vec::new();
        for (id, mut copies) in found {
            if copies.len() == 1 {
                if let Some((_, f)) = copies.pop_first() {
                    files.insert(id, f);
                }
            } else {
                collisions.push((id, copies.into_values().collect()));
            }
        }
        Ok(DiscTextures { files, collisions })
    }

    pub fn files(&self) -> impl Iterator<Item = (&ContentId, &Path)> {
        self.files.iter().map(|(id, p)| (id, p.as_path()))
    }

    pub fn file(&self, id: &ContentId) -> Option<&Path> {
        self.files.get(id).map(PathBuf::as_path)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn block_format(&self, id: &ContentId) -> Option<BlockFormat> {
        let path = self.files.get(id)?;
        if path.extension().and_then(|e| e.to_str()) != Some("xpr") {
            return None;
        }
        let data = std::fs::read(path).ok()?;
        let xpr = XprFile::parse(&data).ok()?;
        let fetch = &xpr.resources.first()?.fetch;
        let identity = [Swizzle::X, Swizzle::Y, Swizzle::Z, Swizzle::W];
        let plane = fetch.surface_2d().is_some_and(|s| s.array_size == 1) && fetch.linear_volume().is_none();
        (plane && fetch.swizzle == identity).then(|| block_format(fetch.format)).flatten()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn layout(&self, id: &ContentId) -> Result<Option<(Shape, bool)>> {
        let Some(path) = self.files.get(id) else { return Ok(None) };
        if path.extension().and_then(|e| e.to_str()) != Some("xpr") {
            return Ok(Some((Shape::Plane, false)));
        }
        let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let xpr = XprFile::parse(&data).with_context(|| format!("parsing {}", path.display()))?;
        let fetch = &xpr.resources.first().with_context(|| format!("{}: no resource", path.display()))?.fetch;
        if fetch.linear_volume().is_some() {
            return Ok(Some((Shape::Volume, false)));
        }
        let surface = fetch.surface_2d().with_context(|| format!("{}: not a 2D or cube texture", path.display()))?;
        let shape = if surface.array_size == 6 { Shape::Cube } else { Shape::Plane };
        Ok(Some((shape, fetch.format == TextureFormat::Dxn)))
    }
}

fn block_format(f: TextureFormat) -> Option<BlockFormat> {
    match f {
        TextureFormat::Dxt1 => Some(BlockFormat::Bc1),
        TextureFormat::Dxt23 => Some(BlockFormat::Bc2),
        TextureFormat::Dxt45 => Some(BlockFormat::Bc3),
        TextureFormat::Dxn => Some(BlockFormat::Bc5),
        _ => None,
    }
}

pub fn decode_xpr(data: &[u8], name: &str) -> Result<Texture> {
    let xpr = XprFile::parse(data).with_context(|| format!("parsing {name}"))?;
    let res = xpr.resources.first().with_context(|| format!("{name}: no resource"))?;
    let decoded =
        texture::decode_resource(data, &xpr, res, true, DxnExport::Raw).with_context(|| format!("decoding {name}"))?;
    if let Some(volume) = res.fetch.linear_volume() {
        let strip = decoded.into_iter().next().with_context(|| format!("{name}: no volume"))?.image;
        let (w, h, d) = (volume.width as usize, volume.height as usize, volume.depth as usize);
        let row = |y: usize, z: usize| &strip.rgba[(y * w * d + z * w) * 4..(y * w * d + z * w + w) * 4];
        let images = (0..d).map(|z| (0..h).flat_map(|y| row(y, z).iter().copied()).collect()).collect();
        let level = Level { width: volume.width, height: volume.height, images };
        return Ok(Texture { shape: Shape::Volume, levels: vec![level], blocks: None, kind: None, source: None });
    }
    let surface = res.fetch.surface_2d().with_context(|| format!("{name}: not a 2D, cube or volume texture"))?;
    let shape = if surface.array_size == 6 { Shape::Cube } else { Shape::Plane };
    let mut levels: Vec<Level> = Vec::new();
    let mut by_level: BTreeMap<u32, Vec<(u32, texture::Image)>> = BTreeMap::new();
    for l in decoded {
        by_level.entry(l.level).or_default().push((l.face, l.image));
    }
    for (_, mut faces) in by_level {
        faces.sort_by_key(|(f, _)| *f);
        let (width, height) = faces.first().map_or((0, 0), |(_, i)| (i.width, i.height));
        if faces.iter().any(|(_, i)| (i.width, i.height) != (width, height)) {
            bail!("{name}: the faces of one level differ in size");
        }
        levels.push(Level { width, height, images: faces.into_iter().map(|(_, i)| i.rgba).collect() });
    }
    let blocks = texture::resource_blocks(data, &xpr, res).with_context(|| format!("blocks of {name}"))?.and_then(
        |(format, levels)| {
            let format = block_format(format)?;
            let levels =
                levels.into_iter().map(|l| BlockLevel { width: l.width, height: l.height, data: l.blocks }).collect();
            Some(BlockChain { format, levels })
        },
    );
    Ok(Texture { shape, levels, blocks, kind: None, source: None })
}

pub fn decode_dds(data: &[u8], name: &str) -> Result<Texture> {
    let dds = DdsFile::parse(data).with_context(|| format!("parsing {name}"))?;
    let format = match dds.format {
        DdsFormat::Dxt1 => TextureFormat::Dxt1,
        DdsFormat::Dxt3 => TextureFormat::Dxt23,
        DdsFormat::Dxt5 => TextureFormat::Dxt45,
    };
    let texels = texture::decode_level(format, dds.level0(data), dds.width, dds.height)
        .with_context(|| format!("decoding {name}"))?;
    let level = Level { width: dds.width, height: dds.height, images: vec![texels.into_iter().flatten().collect()] };
    Ok(Texture { shape: Shape::Plane, levels: vec![level], blocks: None, kind: None, source: None })
}

pub fn decode_path(path: &Path) -> Result<Texture> {
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let name = path.display().to_string();
    match path.extension().and_then(|e| e.to_str()) {
        Some("xpr") => decode_xpr(&data, &name),
        Some("dds") => decode_dds(&data, &name),
        _ => bail!("{name}: not a texture file"),
    }
}

impl Source for DiscTextures {
    fn name(&self) -> &str {
        BASE_NAMESPACE
    }

    fn has(&self, id: &ContentId) -> bool {
        self.files.contains_key(id)
    }

    fn load(&self, id: &ContentId) -> Result<Asset, LoadError> {
        let path = self.files.get(id).ok_or_else(|| LoadError::Missing(id.clone()))?;
        let texture = decode_path(path).map_err(|e| LoadError::Failed { id: id.clone(), message: format!("{e:#}") })?;
        Ok(Asset::Value(Arc::new(texture)))
    }

    fn list(&self, kind: Kind) -> Vec<ContentId> {
        if kind == Kind::Texture { self.files.keys().cloned().collect() } else { Vec::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_of_vault_paths_and_front_end_files() {
        let id = texture_id("worlds\\textures\\Rock_02").unwrap();
        assert_eq!(id.as_str(), "ssx:texture/worlds/textures/rock_02");
        assert_eq!(texture_id("fx/textures/fx_dryice.xpr").unwrap().as_str(), "ssx:texture/fx/textures/fx_dryice");
        let fe = common_texture_id("data/common/fe/views/bootlogo_i6.dds").unwrap();
        assert_eq!(fe.as_str(), "ssx:texture/fe/views/bootlogo_i6");
        assert!(common_texture_id("data/xenon/gameexplorer/x.dds").is_none());
        assert!(texture_id("worlds/tex tures/a b").is_some());
        assert!(texture_id("worlds/../x").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn archives_through_links() {
        let n = std::process::id();
        let root = std::env::temp_dir().join(format!("ssx-disctex-{n}"));
        let _ = std::fs::remove_dir_all(&root);
        let real = root.join("real/cache3");
        let file = |rel: &str| {
            let p = real.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        };
        file("data/xenon/gameexplorer/worlds/textures/rock.xpr");
        file("data/common/fe/views/logo.xpr");
        file("data/common/fe/views/logo.dds");
        file("data/common/fe/assets/icon.dds");
        std::os::unix::fs::symlink(real.join("data/common"), real.join("data/common/fe/loop")).unwrap();
        let big = root.join("linked");
        std::fs::create_dir_all(&big).unwrap();
        std::os::unix::fs::symlink(&real, big.join("cache3")).unwrap();
        let disc = DiscTextures::index(&big).unwrap();
        let ids: Vec<String> = disc.files().map(|(id, _)| id.to_string()).collect();
        assert_eq!(
            ids,
            ["ssx:texture/fe/assets/icon", "ssx:texture/fe/views/logo", "ssx:texture/worlds/textures/rock"],
            "{ids:?}"
        );
        let logo = disc.file(&"ssx:texture/fe/views/logo".parse().unwrap()).unwrap();
        assert!(logo.ends_with("logo.xpr"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
