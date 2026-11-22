use bevy::asset::RenderAssetUsages;
use bevy::camera::{Hdr, RenderTarget};
use bevy::core_pipeline::tonemapping::{DebandDither, Tonemapping};
use bevy::image::Image;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureUsages};
use bevy::ui::IsDefaultUiCamera;
use bevy::window::{PrimaryWindow, WindowRef};
use ssx_assets::port_options::ResolutionScale;

use super::RenderScale;
use crate::render::post::GamePostCamera;
use crate::render::post::fsr::{FsrUpscale, RCAS_SHARPNESS_STOPS, SCENE_FORMAT, ScaledScene};
use crate::ui;

#[derive(Resource)]
pub struct SceneScale {
    pub scale: ResolutionScale,
    image: Option<(Handle<Image>, UVec2)>,
    logged: Option<(ResolutionScale, UVec2)>,
}

impl SceneScale {
    pub fn new(scale: ResolutionScale) -> SceneScale {
        SceneScale { scale, image: None, logged: None }
    }
}

pub(super) fn register(app: &mut App, scale: ResolutionScale) {
    app.insert_resource(SceneScale::new(scale)).add_systems(Update, follow.after(super::render_scale::Retarget));
}

fn scene_image(size: UVec2) -> Image {
    let extent = Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 };
    let mut image = Image::new_uninit(extent, TextureDimension::D2, SCENE_FORMAT, RenderAssetUsages::RENDER_WORLD);
    image.texture_descriptor.usage = TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING;
    image
}

#[derive(Component)]
pub struct ScaledSceneUi;

type SceneCameras<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static mut RenderTarget,
        Has<ScaledScene>,
        Has<IsDefaultUiCamera>,
        Has<ui::UiCamera>,
        Has<Hdr>,
        &'static Msaa,
    ),
    (With<GamePostCamera>, Without<ScaledSceneUi>),
>;
type UiCameras<'w, 's> =
    Query<'w, 's, (Entity, Option<&'static FsrUpscale>, Has<IsDefaultUiCamera>), With<ScaledSceneUi>>;

// PORT(choice): at the screen's resolution a level's UI is drawn by its 3-D camera, after its post-processing and
// Bevy's UI, before its output to the screen (`ui::gpu`'s pass and the screen fade in its `Core3d` schedule); a 2-D
// camera of its own (order 10, sharing the 3-D camera's main texture) cost the render thread its `Core2d` schedule,
// its 2-D passes and a second output to the screen every frame (Mount Eddie traced: 0.36 ms of the render thread's
// schedule, 0.24 of the pool's command buffers). The UI reads the same finished picture and writes the same result,
// which the 3-D camera's output copies to the screen as the 2-D camera's did. A scaled scene still has one: it brings
// the scene up to the screen's resolution and draws the UI there. Bevy's cameras are the port's engine; the console
// drew its UI into its back buffer after the scene.
fn follow(
    mut commands: Commands,
    mut state: ResMut<SceneScale>,
    render: Res<RenderScale>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut images: ResMut<Assets<Image>>,
    mut scenes: SceneCameras,
    uis: UiCameras,
) {
    let Ok(window) = windows.single() else { return };
    let screen = if render.enabled { render.size } else { window.physical_size() };
    let ui_camera = uis.iter().next();
    let Some((cam, mut target, was_scaled, scene_default_ui, scene_ui, hdr, msaa)) = scenes.iter_mut().next() else {
        drop_image(&mut state, &mut images);
        return;
    };
    let has_ui = scene_ui || ui_camera.is_some();
    let scaled = state.scale.factor().is_some() && has_ui && screen.min_element() > 0;
    if !scaled {
        if was_scaled {
            *target = if render.enabled {
                RenderTarget::Image(render.image.clone().into())
            } else {
                RenderTarget::Window(WindowRef::Primary)
            };
            commands.entity(cam).remove::<ScaledScene>();
            info!("display: the 3-D scene at the screen's resolution ({}x{}), no upscaling", screen.x, screen.y);
        }
        if let Some((ui_cam, _, ui_default)) = ui_camera {
            commands.entity(ui_cam).despawn();
            commands.entity(cam).insert(ui::UiCamera);
            if ui_default {
                commands.entity(cam).insert(IsDefaultUiCamera);
            }
        }
        state.logged = None;
        drop_image(&mut state, &mut images);
        return;
    }
    let (w, h) = state.scale.render_size((screen.x, screen.y));
    let size = UVec2::new(w, h);
    let handle = match &state.image {
        Some((handle, s)) if *s == size => handle.clone(),
        _ => {
            let handle = images.add(scene_image(size));
            if let Some((old, _)) = state.image.replace((handle.clone(), size)) {
                images.remove(&old);
            }
            handle
        }
    };
    if target.as_image() != Some(&handle) {
        *target = RenderTarget::Image(handle.clone().into());
    }
    if !was_scaled {
        commands.entity(cam).insert(ScaledScene);
    }
    let fsr = FsrUpscale { input: handle, sharpness: RCAS_SHARPNESS_STOPS };
    match ui_camera {
        Some((ui_cam, current, ui_default)) => {
            if current.is_none_or(|f| f.input != fsr.input) {
                commands.entity(ui_cam).insert(fsr);
            }
            if scene_default_ui && !ui_default {
                commands.entity(cam).remove::<IsDefaultUiCamera>();
                commands.entity(ui_cam).insert(IsDefaultUiCamera);
            }
        }
        None => {
            let mut ui_cam = commands.spawn((
                Camera2d,
                Camera { order: 10, clear_color: ClearColorConfig::None, ..default() },
                Tonemapping::None,
                DebandDither::Disabled,
                *msaa,
                ui::UiCamera,
                ScaledSceneUi,
                fsr,
            ));
            if hdr {
                ui_cam.insert(Hdr);
            }
            if scene_default_ui {
                ui_cam.insert(IsDefaultUiCamera);
                commands.entity(cam).remove::<IsDefaultUiCamera>();
            }
            commands.entity(cam).remove::<ui::UiCamera>();
        }
    }
    if state.logged != Some((state.scale, size)) {
        state.logged = Some((state.scale, size));
        info!(
            "display: the 3-D scene at {}x{} ({}), FSR 1 to the screen's {}x{}",
            size.x, size.y, state.scale, screen.x, screen.y
        );
    }
}

fn drop_image(state: &mut SceneScale, images: &mut Assets<Image>) {
    if let Some((old, _)) = state.image.take() {
        images.remove(&old);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_levels_ui_moves_to_the_scaled_scenes_camera_and_back() {
        let mut app = App::new();
        app.insert_resource(Assets::<Image>::default()).insert_resource(RenderScale::off());
        register(&mut app, ResolutionScale::Quality);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        let cam = app
            .world_mut()
            .spawn((Camera3d::default(), GamePostCamera, Hdr, Msaa::Off, ui::UiCamera, IsDefaultUiCamera))
            .id();
        let ui_cameras = |w: &mut World| w.query_filtered::<Entity, With<ScaledSceneUi>>().iter(w).collect::<Vec<_>>();

        app.update();
        let w = app.world_mut();
        let scaled = ui_cameras(w);
        assert_eq!(scaled.len(), 1);
        let ui_cam = w.entity(scaled[0]);
        let fsr = ui_cam.get::<FsrUpscale>().expect("the scaled scene's FSR");
        assert!(
            ui_cam.contains::<ui::UiCamera>() && ui_cam.contains::<IsDefaultUiCamera>() && ui_cam.contains::<Hdr>()
        );
        assert_eq!(ui_cam.get::<Msaa>(), Some(&Msaa::Off));
        assert_eq!(ui_cam.get::<Camera>().map(|c| c.order), Some(10));
        let scene = w.entity(cam);
        assert!(!scene.contains::<ui::UiCamera>() && !scene.contains::<IsDefaultUiCamera>());
        assert!(scene.contains::<ScaledScene>());
        assert_eq!(scene.get::<RenderTarget>().and_then(|t| t.as_image()), Some(&fsr.input));
        let size = w.resource::<Assets<Image>>().get(&fsr.input).map(|i| i.size());
        assert_eq!(size, Some(UVec2::new(853, 480)), "1280 × 720 ÷ 1.5");

        app.update();
        assert_eq!(ui_cameras(app.world_mut()), scaled);

        app.world_mut().resource_mut::<SceneScale>().scale = ResolutionScale::Native;
        app.update();
        let w = app.world_mut();
        assert!(ui_cameras(w).is_empty() && w.get_entity(scaled[0]).is_err());
        let scene = w.entity(cam);
        assert!(scene.contains::<ui::UiCamera>() && scene.contains::<IsDefaultUiCamera>());
        assert!(!scene.contains::<ScaledScene>());
        assert!(matches!(scene.get::<RenderTarget>(), Some(RenderTarget::Window(WindowRef::Primary))));
        assert!(w.resource::<Assets<Image>>().is_empty(), "the scaled image dropped");
    }
}
