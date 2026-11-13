use std::path::Path;

use anyhow::{Context, Result};
use ssx_assets::track_lighting::LightingPreset;

use super::{Directions, GameLighting, fade, light_state, preset_key, service};

// PORT(choice): a new track without `lighting` is lit by the disc's `default_lighting` (7 of the disc's drops use it,
// each with `default_postfx`; the executable holds the name once, at 0x8219DCC8, with no reference, and loads its key
// 0xFA263423 nowhere); a map names its lighting; the game has no level without a drop.
pub const NEW_TRACK_LIGHTING: &str = "ssx:lighting/default_lighting";

impl GameLighting {
    pub fn for_new_track(
        big_root: &Path,
        name: &str,
        preset: LightingPreset,
        postfx: Option<String>,
        midground: Option<String>,
    ) -> Result<GameLighting> {
        preset.vector("dirLight_Direction").context("preset has no dirLight_Direction")?;
        let Directions { sun, sun_offset, bounce, fill, rim_offset_dir } = Directions::of(&preset);
        let reflect_cube = reflect_cube(big_root, &preset);
        let key = preset_key(&preset.name);
        let mut presets = service::Presets::new();
        presets.insert(key, preset.clone());
        let service = service::LightingService::new(key, &presets);
        let level = ssx_assets::content::GameContent::of(big_root).level_packages(name, &preset);
        let fade = ssx_ui::game::fe::globe_look::LightFade { target: light_state(key, &preset), ..Default::default() };
        Ok(GameLighting {
            track: name.to_string(),
            drop_index: 0,
            drop_key: 0,
            drop_count: 0,
            available: vec![preset.name.clone()],
            postfx,
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
            on_disc_track: false,
            midground,
        })
    }

    pub fn with_map_preset(mut self, big_root: &Path, preset: LightingPreset) -> Result<GameLighting> {
        preset.vector("dirLight_Direction").context("preset has no dirLight_Direction")?;
        let Directions { sun, sun_offset, bounce, fill, rim_offset_dir } = Directions::of(&preset);
        (self.sun, self.sun_offset, self.bounce, self.fill, self.rim_offset_dir) =
            (sun, sun_offset, bounce, fill, rim_offset_dir);
        self.reflect_cube = reflect_cube(big_root, &preset);
        let key = preset_key(&preset.name);
        self.presets.insert(key, preset.clone());
        self.service = service::LightingService::new(key, &self.presets);
        self.level = ssx_assets::content::GameContent::of(big_root).level_packages(&self.track, &preset);
        self.fade = ssx_ui::game::fe::globe_look::LightFade { target: light_state(key, &preset), ..Default::default() };
        if !self.available.contains(&preset.name) {
            self.available.push(preset.name.clone());
        }
        self.preset = preset;
        Ok(self)
    }
}

fn reflect_cube(big_root: &Path, preset: &LightingPreset) -> Option<(std::path::PathBuf, ssx_content::ContentId)> {
    preset
        .textures
        .get("reflectCubeTexture")
        .and_then(|rel| ssx_assets::content::textures::texture_id(rel))
        .filter(|id| ssx_assets::content::GameContent::of(big_root).has(id))
        .map(|id| (big_root.to_path_buf(), id))
}
