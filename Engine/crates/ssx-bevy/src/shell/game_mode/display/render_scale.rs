use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{Camera, Camera2d, ClearColorConfig, RenderTarget};
use bevy::image::{Image, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages};
use bevy::render::view::Tonemapping;
use bevy::window::{PrimaryWindow, WindowRef};
use ssx_assets::display::{DisplaySettings, render_size};

use super::DisplayState;

const PRESENT_LAYER: usize = 9;
const PRESENT_ORDER: isize = 1000;

#[derive(Resource)]
pub struct RenderScale {
    pub image: Handle<Image>,
    pub size: UVec2,
    pub enabled: bool,
    logged: Option<(Option<UVec2>, UVec2)>,
}

#[derive(Component)]
struct PresentCamera;

#[derive(Component)]
struct PresentSprite;

pub(super) fn register(app: &mut App, display: &DisplaySettings, window: (u32, u32)) {
    let size = render_size(window, display.render_height).map_or(UVec2::ONE, |(w, h)| UVec2::new(w, h));
    let image = app.world_mut().resource_mut::<Assets<Image>>().add(target_image(size));
    app.world_mut().spawn((
        Sprite { image: image.clone(), custom_size: Some(Vec2::ONE), ..default() },
        Visibility::Hidden,
        RenderLayers::layer(PRESENT_LAYER),
        PresentSprite,
    ));
    app.world_mut().spawn((
        Camera2d,
        Camera {
            order: PRESENT_ORDER,
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            is_active: false,
            ..default()
        },
        Tonemapping::None,
        Msaa::Off,
        RenderLayers::layer(PRESENT_LAYER),
        PresentCamera,
    ));
    app.insert_resource(RenderScale { image, size, enabled: false, logged: None }).add_systems(Update, follow);
}

fn target_image(size: UVec2) -> Image {
    let extent = Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 };
    let mut image =
        Image::new_uninit(extent, TextureDimension::D2, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::default());
    image.texture_descriptor.usage =
        TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_SRC;
    image.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        ..default()
    });
    image
}

fn follow(
    windows: Query<&Window, With<PrimaryWindow>>,
    display: Res<DisplayState>,
    mut scale: ResMut<RenderScale>,
    mut images: ResMut<Assets<Image>>,
    mut targets: Query<&mut RenderTarget, Without<PresentCamera>>,
    mut present: Query<&mut Camera, With<PresentCamera>>,
    mut sprites: Query<(&mut Sprite, &mut Visibility), With<PresentSprite>>,
) {
    let Ok(window) = windows.single() else { return };
    let physical = window.physical_size();
    let size = render_size((physical.x, physical.y), display.settings.render_height).map(|(w, h)| UVec2::new(w, h));
    let mut replaced = None;
    if let Some(size) = size
        && size != scale.size
    {
        replaced = Some(std::mem::replace(&mut scale.image, images.add(target_image(size))));
        scale.size = size;
    }
    let enabled = size.is_some();
    if scale.logged != Some((size, physical)) {
        scale.logged = Some((size, physical));
        match (size, display.settings.render_height) {
            (Some(s), Some(h)) => info!(
                "display: the frame is rendered at {}x{} and scaled to the window's {}x{} (render height {h})",
                s.x, s.y, physical.x, physical.y
            ),
            _ => info!("display: the frame is rendered at the window's own resolution ({}x{})", physical.x, physical.y),
        }
    }
    scale.enabled = enabled;
    let target = if enabled {
        RenderTarget::Image(scale.image.clone().into())
    } else {
        RenderTarget::Window(WindowRef::Primary)
    };
    for mut current in &mut targets {
        if !same_target(&current, &target) {
            *current = target.clone();
        }
    }
    if let Ok(mut camera) = present.single_mut()
        && camera.is_active != enabled
    {
        camera.is_active = enabled;
    }
    if let Ok((mut sprite, mut visibility)) = sprites.single_mut() {
        if replaced.is_some() {
            sprite.image = scale.image.clone();
        }
        let wanted = if enabled { Visibility::Visible } else { Visibility::Hidden };
        if *visibility != wanted {
            *visibility = wanted;
        }
        if enabled {
            let whole = Some(window.size());
            if sprite.custom_size != whole {
                sprite.custom_size = whole;
            }
        }
    }
    if let Some(old) = replaced {
        images.remove(&old);
    }
}

fn same_target(a: &RenderTarget, b: &RenderTarget) -> bool {
    match (a, b) {
        (RenderTarget::Window(WindowRef::Primary), RenderTarget::Window(WindowRef::Primary)) => true,
        (RenderTarget::Window(WindowRef::Entity(a)), RenderTarget::Window(WindowRef::Entity(b))) => a == b,
        (RenderTarget::Image(a), RenderTarget::Image(b)) => a == b,
        _ => false,
    }
}
