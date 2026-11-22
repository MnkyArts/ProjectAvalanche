use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};

use bevy::asset::{Asset, AssetId, Assets, Handle};
use bevy::image::Image;
use bevy::log::{info, warn};
use bevy::prelude::{App, IntoScheduleConfigs, Local, Plugin, Res, ResMut, Resource, Update};
use bevy::render::render_resource::TextureUsages;
use ssx_assets::content::GameContent;
use ssx_assets::content::dev::DevWatch;
use ssx_content::ContentId;

mod audio;
mod consumers;
mod m4;
mod overlay;
pub(crate) mod ui_content;

pub use overlay::DevOverlay;

pub type Build = Arc<dyn Fn() -> anyhow::Result<Image> + Send + Sync>;

#[derive(Clone)]
pub struct Recipe {
    pub ids: Vec<ContentId>,
    pub build: Build,
}

impl Recipe {
    pub fn new(ids: Vec<ContentId>, build: impl Fn() -> anyhow::Result<Image> + Send + Sync + 'static) -> Self {
        Recipe { ids, build: Arc::new(build) }
    }
}

pub type BuiltImages = Vec<(Image, Recipe)>;

type Rebuilt = Vec<(AssetId<Image>, Vec<ContentId>, anyhow::Result<Image>)>;

static TRACKING: AtomicBool = AtomicBool::new(false);
static LEDGER: Mutex<Vec<(AssetId<Image>, Recipe)>> = Mutex::new(Vec::new());

pub fn tracking() -> bool {
    TRACKING.load(Ordering::Relaxed)
}

pub fn add(images: &mut Assets<Image>, image: Image, recipe: Option<Recipe>) -> Handle<Image> {
    let handle = images.add(image);
    if let Some(r) = recipe {
        track(handle.id(), r);
    }
    handle
}

pub fn track(asset: AssetId<Image>, recipe: Recipe) {
    if tracking() {
        let mut ledger = LEDGER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        ledger.retain(|(a, _)| *a != asset);
        ledger.push((asset, recipe));
    }
}

#[derive(Resource, Default, Clone)]
pub struct ContentReload {
    pub generation: u64,
    pub ids: Option<BTreeSet<ContentId>>,
}

impl ContentReload {
    pub fn touches(&self, id: &ContentId) -> bool {
        self.ids.as_ref().is_none_or(|ids| ids.contains(id))
    }
}

#[derive(Resource, Default)]
struct Replaced(u64);

#[derive(Resource)]
pub(crate) struct ReloadState {
    big_root: PathBuf,
    content: Arc<GameContent>,
    pending: Option<Mutex<Receiver<Rebuilt>>>,
}

pub struct ReloadPlugin {
    pub big_root: PathBuf,
    pub watch: Arc<DevWatch>,
}

impl Plugin for ReloadPlugin {
    fn build(&self, app: &mut App) {
        TRACKING.store(true, Ordering::Relaxed);
        let content = GameContent::of(&self.big_root);
        app.insert_resource(ReloadState { big_root: self.big_root.clone(), content, pending: None })
            .init_resource::<ContentReload>()
            .init_resource::<Replaced>()
            .insert_resource(DevOverlay(self.watch.clone()))
            .add_systems(bevy::prelude::Startup, overlay::spawn)
            .add_systems(Update, (follow_content, apply_images).chain())
            .add_systems(
                Update,
                (
                    consumers::particles,
                    consumers::globe,
                    consumers::ui_images,
                    ui_content::ui_content,
                    consumers::snow_noise,
                    consumers::objects,
                    m4::presentation,
                    audio::follow,
                    overlay::update,
                )
                    .after(follow_content),
            );
        use crate::render::geotags::GeotagMaterial;
        use crate::render::objects::foreground::ForegroundMaterial;
        use crate::render::objects::material::ObjectMaterial;
        use crate::render::terrain::material::TerrainMaterial;
        use crate::rider::ghosts::GhostMaterial;
        use crate::rider::material::CharacterMaterial;
        app.add_systems(
            Update,
            (
                touch::<TerrainMaterial>,
                touch::<ObjectMaterial>,
                touch::<ForegroundMaterial>,
                touch::<CharacterMaterial>,
                touch::<GhostMaterial>,
                touch::<GeotagMaterial>,
            )
                .after(apply_images),
        );
    }
}

fn follow_content(
    mut state: ResMut<ReloadState>,
    mut reload: ResMut<ContentReload>,
    images: Res<Assets<Image>>,
    uploads: Option<Res<crate::render::uploads::LevelUploads>>,
) {
    let content = state.content.clone();
    let generation = content.generation();
    if generation == reload.generation || state.pending.is_some() || uploads.is_some_and(|u| u.pending()) {
        return;
    }
    let ids = content.changes_since(reload.generation);
    *reload = ContentReload { generation, ids: ids.clone() };
    let mut ledger = LEDGER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    ledger.retain(|(id, _)| images.contains(*id));
    let jobs: Vec<(AssetId<Image>, Recipe)> = ledger
        .iter()
        .filter(|(_, r)| r.ids.iter().any(|id| ids.as_ref().is_none_or(|set| set.contains(id))))
        .cloned()
        .collect();
    if jobs.is_empty() {
        return;
    }
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let built = jobs.into_iter().map(|(asset, r)| (asset, r.ids.clone(), (r.build)())).collect();
        let _ = tx.send(built);
    });
    state.pending = Some(Mutex::new(rx));
}

fn apply_images(mut state: ResMut<ReloadState>, mut images: ResMut<Assets<Image>>, mut replaced: ResMut<Replaced>) {
    let got = match &state.pending {
        Some(rx) => match rx.lock().ok().and_then(|r| r.try_recv().ok()) {
            Some(got) => got,
            None => return,
        },
        None => return,
    };
    state.pending = None;
    let mut n = 0;
    for (asset, ids, result) in got {
        match result {
            Ok(mut image) if images.contains(asset) => {
                image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING;
                if images.insert(asset, image).is_ok() {
                    n += 1;
                }
            }
            Ok(_) => {}
            Err(e) => warn!(
                "mod-dev: {} not rebuilt: {e:#}",
                ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
            ),
        }
    }
    if n > 0 {
        replaced.0 += 1;
        info!("mod-dev: {n} images rebuilt");
    }
}

fn touch<M: Asset>(replaced: Res<Replaced>, mut seen: Local<u64>, mut assets: ResMut<Assets<M>>) {
    if replaced.0 == *seen {
        return;
    }
    *seen = replaced.0;
    for _ in assets.iter_mut() {}
}
