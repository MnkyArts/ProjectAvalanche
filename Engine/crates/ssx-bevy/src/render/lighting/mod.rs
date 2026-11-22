pub mod fade;
pub mod game;
pub mod local;
mod map;
pub use map::NEW_TRACK_LIGHTING;
pub mod service;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use bevy::prelude::{Resource, Vec3, Vec4, warn};
use ssx_assets::track_lighting::{LightingPreset, load_track_drops};

#[derive(Resource, Clone, Debug)]
pub struct BigRoot(pub std::path::PathBuf);

#[derive(Clone, Debug)]
pub enum DropSelect {
    Index(usize),
    Preset(String),
    Key(u32),
}

#[derive(Resource, Clone, Debug)]
pub struct GameLighting {
    pub track: String,
    pub drop_index: usize,
    pub drop_key: u32,
    pub drop_count: usize,
    pub available: Vec<String>,
    pub postfx: Option<String>,
    pub preset: LightingPreset,
    pub sun: Vec3,
    pub sun_offset: Vec3,
    pub bounce: Vec3,
    pub fill: Vec3,
    pub rim_offset_dir: Vec3,
    pub reflect_cube: Option<(PathBuf, ssx_content::ContentId)>,
    pub presets: service::Presets,
    pub service: service::LightingService,
    pub fade: ssx_ui::game::fe::globe_look::LightFade,
    driver: fade::Driver,
    pub level: std::sync::Arc<ssx_assets::content::level::LevelPackages>,
    pub on_disc_track: bool,
    pub midground: Option<String>,
}

fn preset_key(name: &str) -> u32 {
    let hex = name.strip_prefix("0x").and_then(|h| u32::from_str_radix(h, 16).ok());
    hex.unwrap_or_else(|| ssx_formats::attrib::fnv1(name.as_bytes()))
}

impl GameLighting {
    pub fn disc_track(&self) -> Option<&str> {
        self.on_disc_track.then_some(self.track.as_str())
    }

    pub fn f(&self, name: &str) -> f32 {
        self.preset.float(name).unwrap_or(0.0)
    }

    pub fn v(&self, name: &str) -> Vec4 {
        self.preset.vector(name).map_or(Vec4::ZERO, Vec4::from_array)
    }

    pub fn replace_presets(&mut self, new: GameLighting) {
        let condition = self.service.condition;
        self.presets = new.presets;
        self.preset = new.preset;
        (self.sun, self.sun_offset, self.bounce, self.fill, self.rim_offset_dir) =
            (new.sun, new.sun_offset, new.bounce, new.fill, new.rim_offset_dir);
        self.postfx = new.postfx;
        self.reflect_cube = new.reflect_cube;
        self.level = new.level;
        if let Some(p) = self.service.collection(&self.presets) {
            let state = light_state(condition, &p);
            self.fade.prev = state.clone();
            self.fade.target = state;
            self.preset = p;
            let d = Directions::of(&self.preset);
            (self.sun, self.sun_offset, self.bounce, self.fill, self.rim_offset_dir) =
                (d.sun, d.sun_offset, d.bounce, d.fill, d.rim_offset_dir);
        }
    }

    pub fn sky(&self) -> Option<&ssx_content::sky::SkyDef> {
        self.level.sky.def()
    }

    pub fn sun_drawn(&self) -> bool {
        self.sky().is_none_or(|s| s.sun)
    }

    pub fn label(&self) -> String {
        format!(
            "drop {}/{} -> {} (post-fx {})",
            self.drop_index + 1,
            self.drop_count,
            self.preset.name,
            self.postfx.as_deref().unwrap_or("-")
        )
    }
}

pub struct Directions {
    pub sun: Vec3,
    pub sun_offset: Vec3,
    pub bounce: Vec3,
    pub fill: Vec3,
    pub rim_offset_dir: Vec3,
}

impl Directions {
    pub fn of(preset: &LightingPreset) -> Self {
        let dir = preset.vector("dirLight_Direction").unwrap_or([0.0, 1.0, 0.0, 0.0]);
        let sun = Vec3::new(dir[0], dir[1], dir[2]).normalize();
        let sun_offset = (sun + Vec3::Y * preset.float("dirLight_Offset").unwrap_or(0.0)).normalize();
        let n = -sun_offset;
        let right = Vec3::Y.cross(n).normalize();
        let up = n.cross(right);
        let a = preset.float("rampBounceOffset").unwrap_or(0.0).to_radians();
        let bounce = -a.sin() * up + a.cos() * n;
        let [x, y] =
            ["fillOffsetPitchAngle", "fillOffsetYawAngle"].map(|k| preset.float(k).unwrap_or(0.0).to_radians());
        let fill_local = Vec3::new(x.cos() * y.sin(), -x.sin(), x.cos() * y.cos());
        let fill = (fill_local.x * right + fill_local.y * up + fill_local.z * n).normalize_or_zero();
        let r = preset.float("rimOffsetAngle").unwrap_or(0.0).to_radians();
        Directions { sun, sun_offset, bounce, fill, rim_offset_dir: Vec3::new(r.sin(), r.cos(), 0.0) }
    }
}

pub fn light_state(key: u32, preset: &LightingPreset) -> ssx_ui::game::fe::globe_look::LightState {
    let d = Directions::of(preset);
    ssx_ui::game::fe::globe_look::LightState {
        sun: d.sun.to_array(),
        sun_offset: d.sun_offset.to_array(),
        bounce: d.bounce.to_array(),
        fill: d.fill.to_array(),
        rim: d.rim_offset_dir.to_array(),
        ..ssx_ui::game::fe::globe_look::LightState::of(key, preset)
    }
}

pub fn load(big_root: &Path, track: &str, select: &DropSelect) -> Result<GameLighting> {
    let drops = load_track_drops(big_root, track)?;
    if drops.is_empty() {
        bail!("{track} has no ge_drop in the vaults");
    }
    let drop_index = match select {
        DropSelect::Index(i) if *i < drops.len() => *i,
        DropSelect::Index(i) => bail!("{track} has {} drops, no drop {}", drops.len(), i + 1),
        DropSelect::Key(key) => {
            drops.iter().position(|d| d.key == *key).with_context(|| format!("{track} has no ge_drop {key:#010x}"))?
        }
        DropSelect::Preset(name) => {
            let want = if name.ends_with("_lighting") { name.clone() } else { format!("{name}_lighting") };
            drops.iter().position(|d| d.lighting.name == want).with_context(|| {
                let names: Vec<&str> = drops.iter().map(|d| d.lighting.name.as_str()).collect();
                format!("{track} has no drop lit by {want} (available: {names:?})")
            })?
        }
    };
    let mut available: Vec<String> = Vec::new();
    for d in &drops {
        if !available.contains(&d.lighting.name) {
            available.push(d.lighting.name.clone());
        }
    }
    let drop = &drops[drop_index];
    let preset = drop.lighting.clone();
    preset.vector("dirLight_Direction").context("preset has no dirLight_Direction")?;
    let Directions { sun, sun_offset, bounce, fill, rim_offset_dir } = Directions::of(&preset);
    let reflect_cube = preset
        .textures
        .get("reflectCubeTexture")
        .and_then(|rel| ssx_assets::content::textures::texture_id(rel))
        .filter(|id| ssx_assets::content::GameContent::of(big_root).has(id))
        .map(|id| (big_root.to_path_buf(), id));
    let mut presets = ssx_assets::career::globe::lighting_presets(big_root, track)
        .inspect_err(|e| warn!("{track}: the lighting collections: {e:#}"))
        .unwrap_or_default();
    let key = preset_key(&preset.name);
    presets.insert(key, preset.clone());
    let service = service::LightingService::new(key, &presets);
    let level = ssx_assets::content::GameContent::of(big_root).level_packages(track, &preset);
    let fade = ssx_ui::game::fe::globe_look::LightFade { target: light_state(key, &preset), ..Default::default() };
    Ok(GameLighting {
        track: track.to_string(),
        drop_index,
        drop_key: drops[drop_index].key,
        drop_count: drops.len(),
        available,
        postfx: drop.postfx.clone(),
        preset,
        sun,
        sun_offset,
        bounce,
        fill,
        rim_offset_dir,
        reflect_cube,
        presets,
        service,
        fade,
        driver: fade::Driver::new(),
        level,
        on_disc_track: true,
        midground: Some(track.to_string()),
    })
}
