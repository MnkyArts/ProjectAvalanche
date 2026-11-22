use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ssx_assets::content::GameContent;
use ssx_assets::terrain_material::{TerrainMaterial as VaultMaterial, load_track_materials};
use ssx_assets::vault_db::MaterialTextures;
use ssx_content::ContentId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnowType {
    Windswept,
    Crusty,
    Wet,
    Groomed,
    Crud,
    Powder,
}

impl SnowType {
    pub const LAYERS: [SnowType; 6] =
        [SnowType::Windswept, SnowType::Crusty, SnowType::Wet, SnowType::Groomed, SnowType::Crud, SnowType::Powder];

    pub fn name(self) -> &'static str {
        match self {
            SnowType::Windswept => "Windswept",
            SnowType::Crusty => "Crusty",
            SnowType::Wet => "Wet",
            SnowType::Groomed => "Groomed",
            SnowType::Crud => "Crud",
            SnowType::Powder => "Powder",
        }
    }
}

#[derive(Clone, Debug)]
pub struct SurfaceLook {
    pub diffuse: ContentId,
    pub normal: ContentId,
    pub tile: f32,
    pub normal_scale: f32,
    pub specular_intensity: f32,
    pub eccentricity: f32,
}

#[derive(Clone, Debug)]
pub struct SnowLayerLook {
    pub snow_type: SnowType,
    pub normal: ContentId,
    pub tile: f32,
    pub specular_intensity: f32,
    pub eccentricity: f32,
}

#[derive(Clone, Debug)]
pub struct TerrainLook {
    pub material: String,
    pub base: [SurfaceLook; 2],
    pub snow: [SnowLayerLook; 6],
    pub scatter: ContentId,
    pub sparkle: ContentId,
    pub reflect_env_exponent: f32,
    pub reflect_env_fresnel: f32,
    pub scatter_tile: f32,
    pub scatter_amount: f32,
    pub planar_blend_offset: f32,
    pub planar_blend_scale: f32,
    pub snow_blend_offset: f32,
    pub snow_blend_point: f32,
    pub snow_blend_range: f32,
    pub snow_fall_direction: [f32; 3],
    pub defaulted: Vec<String>,
}

impl TerrainLook {
    pub fn from_vault(big_root: &Path, m: &VaultMaterial) -> Result<TerrainLook> {
        let mut defaulted = Vec::new();
        let mut float = |name: String, default: f32| {
            m.float(&name).unwrap_or_else(|| {
                defaulted.push(name);
                default
            })
        };
        let content = GameContent::of(big_root);
        let texture = |param: String| -> Result<ContentId> {
            let id = m.texture_id(&param).with_context(|| format!("{}: no {param}", m.name))?;
            anyhow::ensure!(content.has(&id), "{}: {param} = {id} does not exist", m.name);
            Ok(id)
        };
        let mut base = Vec::with_capacity(2);
        for n in 1..=2 {
            base.push(SurfaceLook {
                diffuse: texture(format!("baseSurface{n}DiffuseMap"))?,
                normal: texture(format!("baseSurface{n}NormalMap"))?,
                tile: float(format!("baseSurface{n}Tile"), 0.0),
                normal_scale: float(format!("baseSurface{n}NormalScale"), 0.0),
                specular_intensity: float(format!("baseSurface{n}SpecularIntensity"), 0.0),
                eccentricity: float(format!("baseSurface{n}Eccentricity"), 0.0),
            });
        }
        let mut snow = Vec::with_capacity(6);
        for snow_type in SnowType::LAYERS {
            let name = snow_type.name();
            snow.push(SnowLayerLook {
                snow_type,
                normal: texture(format!("{}NormalMap", name.to_ascii_lowercase()))?,
                tile: float(format!("snowTile{name}"), 1.0),
                specular_intensity: float(format!("snowSpecularIntensity{name}"), 1.0),
                eccentricity: float(format!("snowEccentricity{name}"), 1.0),
            });
        }
        let scatter = texture("scatterNormalMap".into())?;
        let sparkle = texture("sparkleNormalMap".into())?;
        let reflect_env_exponent = float("reflectEnvExponent".into(), 0.0);
        let reflect_env_fresnel = float("reflectEnvFresnel".into(), 0.0);
        let scatter_tile = float("snowScatterTile".into(), 0.0);
        let scatter_amount = float("snowScatterAmount".into(), 0.0);
        let planar_blend_offset = float("threePlanarMappingBlendOffset".into(), 0.0);
        let planar_blend_scale = float("threePlanarMappingBlendScale".into(), 0.0);
        let snow_blend_offset = float("snowBlendOffset".into(), 0.0);
        let snow_blend_point = float("snowBlendPoint".into(), 0.0);
        let snow_blend_range = float("snowBlendRange".into(), 0.0);
        let snow_fall_direction = match m.vectors.get("snowFallDirection").map(Vec::as_slice) {
            Some(&[x, y, z, ..]) => [x, y, z],
            _ => {
                defaulted.push("snowFallDirection".into());
                [0.0, 0.0, 0.0]
            }
        };
        Ok(TerrainLook {
            material: m.name.clone(),
            base: base.try_into().unwrap_or_else(|_| unreachable!()),
            snow: snow.try_into().unwrap_or_else(|_| unreachable!()),
            scatter,
            sparkle,
            reflect_env_exponent,
            reflect_env_fresnel,
            scatter_tile,
            scatter_amount,
            planar_blend_offset,
            planar_blend_scale,
            snow_blend_offset,
            snow_blend_point,
            snow_blend_range,
            snow_fall_direction,
            defaulted,
        })
    }
}

pub struct TrackLooks {
    pub looks: Vec<TerrainLook>,
    pub by_file: BTreeMap<String, usize>,
}

impl TrackLooks {
    pub fn load(big_root: &Path, track: &str) -> Result<TrackLooks> {
        let materials = load_track_materials(big_root, track)?;
        let mut looks: Vec<TerrainLook> = Vec::new();
        let mut by_file = BTreeMap::new();
        for (file, m) in &materials {
            let index = match looks.iter().position(|l| l.material == m.name) {
                Some(i) => i,
                None => {
                    looks.push(TerrainLook::from_vault(big_root, m)?);
                    looks.len() - 1
                }
            };
            by_file.insert(file.trim_end_matches(".subd").to_string(), index);
        }
        Ok(TrackLooks { looks, by_file })
    }

    pub fn summary(&self) -> String {
        let stem = |id: &ContentId| id.path().rsplit('/').next().unwrap_or_default().to_string();
        let mut out = String::from("materials (vault):");
        for (i, look) in self.looks.iter().enumerate() {
            let files = self.by_file.values().filter(|&&l| l == i).count();
            let [b1, b2] = &look.base;
            let surface = |s: &SurfaceLook| format!("{} {}/m", stem(&s.diffuse).trim_end_matches("_diff_spec"), s.tile);
            out += &format!("\n  {} x{files}: base1 {} | base2 {}", look.material, surface(b1), surface(b2));
            if !look.defaulted.is_empty() {
                out += &format!(" | unset: {}", compact_names(&look.defaulted));
            }
        }
        if let Some(look) = self.looks.first() {
            let layers = look
                .snow
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let file = stem(&l.normal);
                    let file = file.trim_start_matches("snow_").trim_end_matches("_normal");
                    format!("{i} {}={file}", l.snow_type.name().to_ascii_lowercase())
                })
                .collect::<Vec<_>>();
            out += &format!("\nsnow layers: {}", layers.join(" "));
        }
        out + "\n" + crate::render::terrain::material::SHADER_STATUS
    }
}

pub fn find_big_root(track_dir: &Path) -> Option<PathBuf> {
    find_big_root_from(track_dir, std::env::current_dir().ok().as_deref())
}

fn find_big_root_from(track_dir: &Path, cwd: Option<&Path>) -> Option<PathBuf> {
    let is_root = |p: &Path| p.join("cache1/data/xenon/attribdb").is_dir();
    #[cfg(windows)]
    let track_dir = std::path::absolute(track_dir).unwrap_or_else(|_| track_dir.to_path_buf());
    #[cfg(not(windows))]
    let track_dir = std::fs::canonicalize(track_dir).unwrap_or_else(|_| track_dir.to_path_buf());
    if let Some(root) = track_dir.ancestors().find(|p| is_root(p)) {
        return Some(root.to_path_buf());
    }
    // PORT(choice): a track folder outside any archive root (a DLC track's, `<DLC>/<package>/data/xenon/gameexplorer/
    // <track>`) takes the development tree's `Game-Data/Extracted/big` above the working directory: for the asset
    // viewer run there without `--big-root`. No loader relies on it: the game and the launcher's viewer always pass the
    // root, and on a player's computer no such folder lies above the working directory (0.4.2-dev's terrain loader
    // searched here with the root given, and a player's Mount Eddie had no proxy and no snow-depth displacement). The
    // console reads its archives from its disc and mounts each package at a device of its own (`DLC%04d:`,
    // `DlcContent_Mount` 0x82ACFDB0 through `XamContentCreateEx`); it searches no folders.
    cwd?.ancestors().map(|dir| dir.join("Game-Data/Extracted/big")).find(|p| is_root(p))
}

fn compact_names(names: &[String]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut rest: Vec<&String> = names.iter().collect();
    for prefix in ["snowTile", "snowSpecularIntensity", "snowEccentricity"] {
        let all: Vec<String> = SnowType::LAYERS.iter().map(|t| format!("{prefix}{}", t.name())).collect();
        if all.iter().all(|n| names.contains(n)) {
            out.push(format!("{prefix}<all types>"));
            rest.retain(|n| !all.contains(n));
        }
    }
    out.extend(rest.into_iter().cloned());
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_track_folder_finds_its_own_root_and_a_dlc_track_only_the_development_trees() {
        let top = std::env::temp_dir().join(format!("ssx-big-root-{}", std::process::id()));
        let disc = top.join("GameData/big");
        let disc_track = disc.join("data0/data/xenon/gameexplorer/alps0101");
        let dlc_track = top.join("DLC/dlc_paid_mnt_eddie/data/xenon/gameexplorer/rockies0706");
        let dev = top.join("dev/Game-Data/Extracted/big");
        let dev_cwd = top.join("dev/Engine");
        for dir in [disc.join("cache1/data/xenon/attribdb"), disc_track.clone(), dlc_track.clone()] {
            std::fs::create_dir_all(dir).expect("the test's folders");
        }
        std::fs::create_dir_all(dev.join("cache1/data/xenon/attribdb")).expect("the test's folders");
        std::fs::create_dir_all(&dev_cwd).expect("the test's folders");
        let disc = std::fs::canonicalize(&disc).expect("the root's canonical path");
        assert_eq!(find_big_root_from(&disc_track, Some(&dev_cwd)), Some(disc.clone()));
        assert_eq!(find_big_root_from(&disc_track, None), Some(disc));
        assert_eq!(find_big_root_from(&dlc_track, Some(&dev_cwd)), Some(dev));
        assert_eq!(find_big_root_from(&dlc_track, None), None);
        std::fs::remove_dir_all(&top).expect("the test's folders removed");
    }
}
