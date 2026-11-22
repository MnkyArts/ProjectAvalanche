use std::time::{Duration, Instant};

use anyhow::Result;
use bevy::ecs::system::SystemParam;
use bevy::prelude::{Assets, Handle, Image, Res, ResMut, Resource, debug, info, warn};
use ssx_assets::postfx::{self, ColourCube, ColourCubeBlend, ColourStack, CubeBuild, TRACK_STACK};
use ssx_assets::postfx_state::CubeEntry;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    stack: u32,
    mode: u32,
    group: String,
    opacity: u8,
}

struct Sources {
    stacks: Vec<ColourStack>,
    track: Vec<Key>,
    track_cube: ColourCube,
    over: ColourCube,
}

#[derive(Resource)]
pub struct FrameCube {
    sources: Sources,
    built: Vec<Key>,
    shown: Vec<String>,
    failed: Option<Vec<Key>>,
}

fn keys<'a>(stacks: &[ColourStack], entries: impl Iterator<Item = (u32, u32, &'a str, f32)>) -> Vec<Key> {
    let mut out: Vec<Key> = entries
        .filter(|(stack, _, group, _)| {
            stacks.iter().find(|s| s.index == *stack).is_none_or(|s| s.xml.groups.iter().any(|g| g.name == *group))
        })
        .map(|(stack, mode, group, weight)| Key {
            stack,
            mode,
            group: group.to_string(),
            opacity: ((weight * 255.0) as i32).clamp(0, 255) as u8,
        })
        .collect();
    out.sort();
    out
}

fn blends(keys: &[Key]) -> Vec<ColourCubeBlend> {
    keys.iter()
        .map(|k| ColourCubeBlend {
            stack: k.stack,
            mode: k.mode,
            group: k.group.clone(),
            weight: f32::from(k.opacity) / 255.0,
        })
        .collect()
}

impl Sources {
    fn build(&self, keys: &[Key]) -> Result<ColourCube> {
        if keys == self.track {
            return Ok(self.track_cube.clone());
        }
        let track_entries = |k: &[Key]| k.iter().filter(|k| k.stack == TRACK_STACK).cloned().collect::<Vec<_>>();
        let over = (track_entries(keys) == track_entries(&self.track)).then_some(&self.over);
        Ok(postfx::build_cube_from(&self.stacks, &blends(keys), over)?.cube)
    }
}

impl FrameCube {
    pub fn load(root: &std::path::Path, track: &[ColourCubeBlend], build: &CubeBuild) -> Result<FrameCube> {
        let stacks = postfx::load_stacks(root)?;
        let track = keys(&stacks, track.iter().map(|b| (b.stack, b.mode, b.group.as_str(), b.weight)));
        // PORT(choice): a package's colour cube given as a PNG strip for the drop's preset stands for the track
        // stack's output: while the frame's track-stack entries are the preset's own, the other stacks run over the
        // strip with the frame's entries; when they differ the disc's stacks build the whole cube. Packages and their
        // strips are the port's; the console builds every frame's cube from its stacks.
        let over = if build.strip { build.cube.clone() } else { postfx::track_stack_cube(&stacks, &blends(&track))? };
        Ok(FrameCube {
            built: track.clone(),
            sources: Sources { stacks, track, track_cube: build.cube.clone(), over },
            shown: Vec::new(),
            failed: None,
        })
    }

    pub fn update(&mut self, entries: &[CubeEntry], lut: &Handle<Image>, images: &mut Assets<Image>) {
        let keys = keys(
            &self.sources.stacks,
            entries.iter().map(|e| (u32::from(e.stack), u32::from(e.mode), e.group.as_str(), e.weight)),
        );
        if keys == self.built || self.failed.as_ref() == Some(&keys) {
            return;
        }
        let started = Instant::now();
        let image = self.sources.build(&keys).map(|cube| super::lut_image(&cube));
        self.take(keys, image, started.elapsed(), lut, images);
    }

    fn take(
        &mut self,
        keys: Vec<Key>,
        image: Result<Image>,
        took: Duration,
        lut: &Handle<Image>,
        images: &mut Assets<Image>,
    ) {
        match image {
            Ok(image) => {
                if let Err(e) = images.insert(lut, image) {
                    warn!("the frame's colour cube: the LUT image is gone ({e})");
                    return;
                }
                debug!("colour cube of {} entries built in {took:?}", keys.len());
                (self.built, self.failed) = (keys, None);
                let shown = self.others().map(|k| k.group.clone()).collect::<Vec<_>>();
                if shown != self.shown {
                    info!("the frame's colour cube: {} (built in {took:?})", self.describe());
                    self.shown = shown;
                }
            }
            Err(e) => {
                warn!("the frame's colour cube ({keys:?}) not built: {e:#}; the last one stays");
                self.failed = Some(keys);
            }
        }
    }

    fn others(&self) -> impl Iterator<Item = &Key> {
        self.built.iter().filter(|k| k.opacity > 0 && !self.sources.track.contains(k))
    }

    pub fn describe(&self) -> String {
        let shown: Vec<String> = self.others().map(|k| format!("{} {}", k.group, k.opacity)).collect();
        if shown.is_empty() { "the drop's".to_string() } else { format!("CUBE {}", shown.join(", ")) }
    }
}

#[derive(SystemParam)]
pub struct CubeTarget<'w> {
    cube: Option<ResMut<'w, FrameCube>>,
    settings: Option<Res<'w, super::PostSettings>>,
    levels: Res<'w, super::PostLevels>,
    images: ResMut<'w, Assets<Image>>,
}

impl CubeTarget<'_> {
    pub fn frame(&mut self, entries: &[CubeEntry]) -> String {
        let (Some(cube), Some(settings)) = (self.cube.as_mut(), self.settings.as_ref()) else { return String::new() };
        if !self.levels.post {
            return String::new();
        }
        cube.update(entries, &settings.lut, &mut self.images);
        format!(" | colour cube: {}", cube.describe())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(index: u32, groups: &[&str]) -> ColourStack {
        let groups: String = groups
            .iter()
            .map(|g| {
                format!(
                    r#"<group name="{g}" enabled="false" opacity="255"><layer name="l" enabled="true" type="BrightnessContrast" blendmode="Normal" opacity="255"><parameter name="brightness" value="40"/><parameter name="contrast" value="0"/></layer></group>"#
                )
            })
            .collect();
        let text = format!(r#"<?xml version="1.0"?><stack name="s{index}">{groups}</stack>"#);
        ColourStack { index, file: Default::default(), xml: postfx::parse_stack(&text).unwrap() }
    }

    #[test]
    fn entries_of_groups_a_stack_lacks_change_nothing() {
        let stacks = [stack(0, &["day"]), stack(1, &["awesome_night"])];
        let k = keys(&stacks, [(1, 0, "awesome_night", 0.5), (0, 0, "", 0.0), (0, 0, "day", 1.0)].into_iter());
        assert_eq!(
            k.iter().map(|k| (k.group.as_str(), k.opacity)).collect::<Vec<_>>(),
            [("day", 255), ("awesome_night", 127)]
        );
    }
}
