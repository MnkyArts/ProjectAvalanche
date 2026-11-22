use std::collections::VecDeque;

use bevy::asset::{Asset, AssetId};
use bevy::prelude::*;
use bevy::render::mesh::RenderMesh;
use bevy::render::render_asset::RenderAsset;
use bevy::render::texture::GpuImage;

use crate::render::reload::Recipe;

// PORT(choice): while the loading screen covers a new level, at most 8 MiB of its terrain, proxy and object meshes
// (their vertices and indices, `RenderMesh::byte_len`) enter the mesh store a frame. alps0101's 2 161 such meshes are
// 65.8 MiB; added at once, the extraction's copies took 81 ms and the mesh allocator's writes 193 ms of the frame after
// the spawn on the development laptop (`trace_chrome`), about 4 ms a MiB split over the two threads. At 8 MiB they
// take 9 frames of 23–49 ms (the level's 31 pipelines compiling beside them) and the level comes up 0.7 s after its
// spawn (1.3 s with them at once). The console streams its levels from the disc into memory it owns; a PC copies them
// into the graphics driver's memory, which Bevy does a frame at a time.
const MESH_BYTES_PER_FRAME: usize = 8 << 20;

// PORT(choice): while the loading screen covers a new level, at most 8 MiB of its terrain and object textures (their
// data as the GPU takes it, `GpuImage::byte_len`) enter the image store a frame, before its spawn. Added at once they
// went up in the frame after the spawn with every other new texture: 76 ms of its `prepare_assets::<GpuImage>` for
// alps0101's 244 textures, 107.7 MiB (development laptop, `trace_chrome`), about 0.7 ms a MiB; 93 of them, 39.9 MiB,
// are the level's terrain and object textures. The console streams its levels from the disc into memory it owns; a PC
// copies them into the graphics driver's memory.
const IMAGE_BYTES_PER_FRAME: usize = 8 << 20;

const SETTLE_FRAMES: u32 = 3;

type Queued<A> = (AssetId<A>, A, usize);

#[derive(Resource, Default)]
pub struct LevelUploads {
    loading: bool,
    meshes: VecDeque<Queued<Mesh>>,
    images: VecDeque<Queued<Image>>,
    since_insert: u32,
    since_image: u32,
    totals: Totals,
}

#[derive(Default, Clone, Copy)]
struct Totals {
    meshes: usize,
    mesh_bytes: usize,
    images: usize,
    image_bytes: usize,
    frames: u32,
}

impl LevelUploads {
    pub fn add_mesh(&mut self, meshes: &mut Assets<Mesh>, mesh: Mesh) -> Handle<Mesh> {
        if !self.loading {
            return meshes.add(mesh);
        }
        let handle = meshes.reserve_handle();
        let bytes = <RenderMesh as RenderAsset>::byte_len(&mesh).unwrap_or(0);
        self.meshes.push_back((handle.id(), mesh, bytes));
        self.totals.meshes += 1;
        self.totals.mesh_bytes += bytes;
        handle
    }

    pub fn add_image(&mut self, images: &mut Assets<Image>, image: Image, recipe: Option<Recipe>) -> Handle<Image> {
        if !self.loading {
            return crate::render::reload::add(images, image, recipe);
        }
        let handle = images.reserve_handle();
        if let Some(r) = recipe {
            crate::render::reload::track(handle.id(), r);
        }
        let bytes = <GpuImage as RenderAsset>::byte_len(&image).unwrap_or(0);
        self.images.push_back((handle.id(), image, bytes));
        self.since_image = 0;
        self.totals.images += 1;
        self.totals.image_bytes += bytes;
        handle
    }

    pub fn add_image_at(&mut self, images: &mut Assets<Image>, id: AssetId<Image>, image: Image) {
        if !self.loading {
            if images.insert(id, image).is_err() {
                warn!("level uploads: the fixed texture {id:?} could not be replaced");
            }
            return;
        }
        let bytes = <GpuImage as RenderAsset>::byte_len(&image).unwrap_or(0);
        self.images.push_back((id, image, bytes));
        self.since_image = 0;
        self.totals.images += 1;
        self.totals.image_bytes += bytes;
    }

    pub fn pending(&self) -> bool {
        !self.meshes.is_empty() || !self.images.is_empty() || self.since_insert < SETTLE_FRAMES
    }

    pub fn textures_in(&self) -> bool {
        self.images.is_empty() && self.since_image > 0
    }
}

pub enum PartImages {
    Built(crate::render::reload::BuiltImages),
    Queued(Vec<Handle<Image>>),
}

impl PartImages {
    pub fn queue(&mut self, uploads: &mut LevelUploads, images: &mut Assets<Image>) {
        if let PartImages::Built(built) = self {
            let handles =
                std::mem::take(built).into_iter().map(|(image, recipe)| uploads.add_image(images, image, Some(recipe)));
            *self = PartImages::Queued(handles.collect());
        }
    }

    pub fn into_handles(self, images: &mut Assets<Image>) -> Vec<Handle<Image>> {
        match self {
            PartImages::Built(built) => built
                .into_iter()
                .map(|(image, recipe)| crate::render::reload::add(images, image, Some(recipe)))
                .collect(),
            PartImages::Queued(handles) => handles,
        }
    }
}

pub enum LevelImage {
    Built(Box<(Image, Option<Recipe>)>),
    Queued(Handle<Image>),
}

impl LevelImage {
    pub fn built(image: Image, recipe: Option<Recipe>) -> LevelImage {
        LevelImage::Built(Box::new((image, recipe)))
    }

    pub fn queue(&mut self, uploads: &mut LevelUploads, images: &mut Assets<Image>) {
        match std::mem::replace(self, LevelImage::Queued(Handle::default())) {
            LevelImage::Built(built) => {
                let (image, recipe) = *built;
                *self = LevelImage::Queued(uploads.add_image(images, image, recipe));
            }
            queued => *self = queued,
        }
    }

    pub fn into_handle(self, images: &mut Assets<Image>) -> Handle<Image> {
        match self {
            LevelImage::Built(built) => {
                let (image, recipe) = *built;
                crate::render::reload::add(images, image, recipe)
            }
            LevelImage::Queued(handle) => handle,
        }
    }
}

pub fn add_mesh(uploads: Option<&mut LevelUploads>, meshes: &mut Assets<Mesh>, mesh: Mesh) -> Handle<Mesh> {
    match uploads {
        Some(u) => u.add_mesh(meshes, mesh),
        None => meshes.add(mesh),
    }
}

pub struct LevelUploadsPlugin;

impl Plugin for LevelUploadsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LevelUploads>().add_systems(PreUpdate, trickle);
    }
}

pub fn begin(world: &mut World) {
    if let Some(mut u) = world.get_resource_mut::<LevelUploads>() {
        u.loading = true;
        u.totals = Totals::default();
        u.since_insert = 0;
        u.since_image = 1;
    }
}

pub fn finish(uploads: &mut LevelUploads) -> String {
    uploads.loading = false;
    let t = uploads.totals;
    let mib = |b: usize| b as f64 / f64::from(1 << 20);
    format!(
        "level uploads: {} meshes, {:.1} MiB and {} textures, {:.1} MiB over {} frames (at most {} and {} MiB a frame)",
        t.meshes,
        mib(t.mesh_bytes),
        t.images,
        mib(t.image_bytes),
        t.frames,
        MESH_BYTES_PER_FRAME >> 20,
        IMAGE_BYTES_PER_FRAME >> 20
    )
}

pub fn reset(world: &mut World) {
    if let Some(mut u) = world.get_resource_mut::<LevelUploads>() {
        u.loading = false;
        u.meshes.clear();
        u.images.clear();
    }
}

fn trickle(mut uploads: ResMut<LevelUploads>, mut meshes: ResMut<Assets<Mesh>>, mut images: ResMut<Assets<Image>>) {
    let u = &mut *uploads;
    u.since_image = if u.images.is_empty() { u.since_image.saturating_add(1) } else { 0 };
    if u.meshes.is_empty() && u.images.is_empty() {
        u.since_insert = u.since_insert.saturating_add(1);
        return;
    }
    u.since_insert = 0;
    move_within(&mut u.meshes, &mut meshes, MESH_BYTES_PER_FRAME);
    move_within(&mut u.images, &mut images, IMAGE_BYTES_PER_FRAME);
    u.totals.frames += 1;
}

fn move_within<A: Asset>(queue: &mut VecDeque<Queued<A>>, store: &mut Assets<A>, budget: usize) {
    let mut written = 0;
    while let Some(bytes) = queue.front().map(|m| m.2)
        && (written == 0 || written + bytes <= budget)
        && let Some((id, asset, _)) = queue.pop_front()
    {
        if store.insert(id, asset).is_err() {
            debug!("level uploads: a queued {}'s handle was dropped", std::any::type_name::<A>());
        }
        written += bytes;
    }
}

#[cfg(test)]
mod tests {
    use bevy::asset::RenderAssetUsages;
    use bevy::ecs::system::RunSystemOnce;
    use bevy::mesh::PrimitiveTopology;
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    use super::*;

    fn mesh(mib: usize) -> Mesh {
        let n = mib * (1 << 20) / 12;
        Mesh::new(PrimitiveTopology::PointList, RenderAssetUsages::default())
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, vec![[0.0_f32; 3]; n])
    }

    fn image(mib: usize) -> Image {
        let size = Extent3d { width: 512, height: (mib << 20) as u32 / 2048, depth_or_array_layers: 1 };
        Image::new_fill(size, TextureDimension::D2, &[0; 4], TextureFormat::Rgba8Unorm, RenderAssetUsages::default())
    }

    fn world() -> World {
        let mut world = World::new();
        world.init_resource::<Assets<Mesh>>();
        world.init_resource::<Assets<Image>>();
        world.init_resource::<LevelUploads>();
        world
    }

    fn add(world: &mut World, mib: usize) -> Handle<Mesh> {
        world.resource_scope(|world, mut uploads: Mut<LevelUploads>| {
            uploads.add_mesh(&mut world.resource_mut::<Assets<Mesh>>(), mesh(mib))
        })
    }

    fn add_texture(world: &mut World, mib: usize) -> Handle<Image> {
        world.resource_scope(|world, mut uploads: Mut<LevelUploads>| {
            uploads.add_image(&mut world.resource_mut::<Assets<Image>>(), image(mib), None)
        })
    }

    fn frame(world: &mut World) {
        world.run_system_once(trickle).expect("trickle runs");
    }

    #[test]
    fn a_loading_levels_meshes_go_in_within_the_budget() {
        let mut world = world();
        let budget = MESH_BYTES_PER_FRAME >> 20;
        assert_eq!(<RenderMesh as RenderAsset>::byte_len(&mesh(3)), Some(3 << 20));
        begin(&mut world);
        let sizes = [budget / 2, budget / 2, budget / 2, budget + 1, 1];
        let handles: Vec<_> = sizes.into_iter().map(|mib| add(&mut world, mib)).collect();
        let present = |world: &World| handles.iter().filter(|h| world.resource::<Assets<Mesh>>().contains(*h)).count();
        assert_eq!(present(&world), 0);
        for want in [2, 3, 4, 5] {
            frame(&mut world);
            assert_eq!(present(&world), want);
            assert!(world.resource::<LevelUploads>().pending());
        }
        for _ in 0..SETTLE_FRAMES {
            assert!(world.resource::<LevelUploads>().pending());
            frame(&mut world);
        }
        assert!(!world.resource::<LevelUploads>().pending());
        let line = finish(&mut world.resource_mut::<LevelUploads>());
        assert_eq!(
            line,
            format!(
                "level uploads: 5 meshes, {}.0 MiB and 0 textures, 0.0 MiB over 4 frames (at most {budget} and {} MiB \
                 a frame)",
                sizes.iter().sum::<usize>(),
                IMAGE_BYTES_PER_FRAME >> 20
            )
        );
        let later = add(&mut world, 1);
        assert!(world.resource::<Assets<Mesh>>().contains(&later));
    }

    #[test]
    fn a_loading_levels_textures_go_in_within_their_budget_before_its_spawn() {
        let mut world = world();
        let budget = IMAGE_BYTES_PER_FRAME >> 20;
        assert_eq!(<GpuImage as RenderAsset>::byte_len(&image(3)), Some(3 << 20));
        let textures_in = |world: &World| world.resource::<LevelUploads>().textures_in();
        begin(&mut world);
        assert!(textures_in(&world));
        let sizes = [budget / 2, budget / 2, budget + 1, 1];
        let textures: Vec<_> = sizes.into_iter().map(|mib| add_texture(&mut world, mib)).collect();
        let present =
            |world: &World| textures.iter().filter(|h| world.resource::<Assets<Image>>().contains(*h)).count();
        assert_eq!(present(&world), 0);
        for want in [2, 3, 4] {
            assert!(!textures_in(&world));
            frame(&mut world);
            assert_eq!(present(&world), want);
        }
        assert!(!textures_in(&world));
        frame(&mut world);
        assert!(textures_in(&world));
        let mesh_budget = MESH_BYTES_PER_FRAME >> 20;
        let meshes = [add(&mut world, mesh_budget), add(&mut world, mesh_budget)];
        let meshes_in = |world: &World| meshes.iter().filter(|h| world.resource::<Assets<Mesh>>().contains(*h)).count();
        for want in [1, 2] {
            assert!(world.resource::<LevelUploads>().pending());
            frame(&mut world);
            assert_eq!(meshes_in(&world), want);
        }
        for _ in 0..SETTLE_FRAMES {
            assert!(world.resource::<LevelUploads>().pending());
            frame(&mut world);
        }
        assert!(!world.resource::<LevelUploads>().pending());
        let line = finish(&mut world.resource_mut::<LevelUploads>());
        let (mesh_mib, mib) = (2 * mesh_budget, 2 * (budget / 2) + budget + 2);
        let want = format!("level uploads: 2 meshes, {mesh_mib}.0 MiB and 4 textures, {mib}.0 MiB over 5 frames");
        assert!(line.starts_with(&want), "{line}");
        let later = add_texture(&mut world, 1);
        assert!(world.resource::<Assets<Image>>().contains(&later));
    }

    #[test]
    fn a_fixed_handles_texture_and_a_parts_images_queue_before_the_spawn() {
        const FIXED: Handle<Image> = bevy::asset::uuid_handle!("6f0e8a51-3c2d-4b7e-9a16-2d5c8e7f1b03");
        let mut world = world();
        let pixel = |world: &World| world.resource::<Assets<Image>>().get(&FIXED).map(|i| i.width());
        world.resource_mut::<Assets<Image>>().insert(FIXED.id(), image(1)).unwrap();
        begin(&mut world);
        let mut part = LevelImage::built(image(2), None);
        world.resource_scope(|world, mut uploads: Mut<LevelUploads>| {
            let mut images = world.resource_mut::<Assets<Image>>();
            let wide = Image::new_fill(
                Extent3d { width: 1024, height: 4, depth_or_array_layers: 1 },
                TextureDimension::D2,
                &[0; 4],
                TextureFormat::Rgba8Unorm,
                RenderAssetUsages::default(),
            );
            uploads.add_image_at(&mut images, FIXED.id(), wide);
            part.queue(&mut uploads, &mut images);
        });
        let LevelImage::Queued(handle) = &part else { panic!("the part's image was not queued") };
        let handle = handle.clone();
        assert_eq!(pixel(&world), Some(512));
        assert!(!world.resource::<LevelUploads>().textures_in());
        frame(&mut world);
        assert_eq!(pixel(&world), Some(1024));
        assert!(world.resource::<Assets<Image>>().contains(&handle));
        frame(&mut world);
        assert!(world.resource::<LevelUploads>().textures_in());
        assert_eq!(part.into_handle(&mut world.resource_mut::<Assets<Image>>()), handle);
    }

    #[test]
    fn a_level_torn_down_drops_its_queue() {
        let mut world = world();
        begin(&mut world);
        let texture = add_texture(&mut world, 1);
        let mesh = add(&mut world, 1);
        reset(&mut world);
        for _ in 0..SETTLE_FRAMES {
            frame(&mut world);
        }
        assert!(!world.resource::<LevelUploads>().pending());
        assert!(!world.resource::<Assets<Image>>().contains(&texture));
        assert!(!world.resource::<Assets<Mesh>>().contains(&mesh));
        let after = add_texture(&mut world, 1);
        assert!(world.resource::<Assets<Image>>().contains(&after));
    }
}
