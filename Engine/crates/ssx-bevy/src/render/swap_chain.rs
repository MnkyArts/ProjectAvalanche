use bevy::app::SubApp;
use bevy::camera::{CameraOutputMode, NormalizedRenderTarget};
use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::schedule::{ScheduleLabel, SingleThreadedExecutor};
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bevy::render::camera::{SortedCameras, sort_cameras};
use bevy::render::renderer::render_system;
use bevy::render::sync_world::MainEntity;
use bevy::render::view::{ExtractedWindow, SurfaceData, prepare_windows};
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderScheduleOrder, RenderSystems};

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct NextFrameImages;

pub struct SwapChainPlugin;

impl Plugin for SwapChainPlugin {
    fn build(&self, app: &mut App) {
        let Some(render) = app.get_sub_app_mut(RenderApp) else { return };
        let world = render.world_mut();
        if world.get_resource::<RenderScheduleOrder>().is_none_or(|o| !o.labels.contains(&Render.intern())) {
            warn!("render: no Render schedule in the render app's order; each frame takes its own swap-chain images");
            return;
        }
        if world.register_component_hooks::<ExtractedWindow>().try_on_remove(release_on_remove).is_none() {
            warn!("render: ExtractedWindow has an on_remove hook already; each frame takes its own swap-chain images");
            return;
        }
        // PORT(choice): the next frame's swap-chain image is taken on the render thread right after the frame's
        // present instead of in the next frame's `prepare_windows` on a compute-pool thread. Bevy 0.20's default pools
        // give the compute pool 2 threads with 6 or 4 CPUs and 8 with 16 (`TaskPoolOptions`: io and async compute 25 %
        // each, rounded); the main world's systems and the render world's share them, so the wait for the GPU (Mount
        // Eddie, 6 P-cores, the dev laptop: 1.83–1.98 ms a frame, 1.81–1.96 of it inside the main update) took one
        // of the two from the update. The render thread waits instead, after its frame's work, where it holds nothing
        // of the render world's; the next extract, and with it the main world's next update, waits for the image.
        // Measured there with the compute pool at every CPU but one, at most four (`level::task_pool_plugin`): p99
        // 13.90–13.94 -> 10.46–10.54 ms, a main update's start to its frame's present 15.4 -> 13.2 ms; alone, without
        // the larger pool, the frame was 0.14 ms longer (the render world's systems then meet the update's on two
        // threads). Where the render thread holds the frame (all 16 CPUs there) the acquire is on its path: with
        // Bevy's 8 compute threads the heli scene's frame 7.95 -> 8.35 ms; with four, alps0101's 7.94–8.04 -> 7.29–7.35
        // ms and p99 11.83–12.16 -> 10.26–10.55 against main.
        // Engine mechanics: the console's renderer had no such schedules or pool.
        if let Some(mut order) = world.get_resource_mut::<RenderScheduleOrder>() {
            order.insert_after(Render, NextFrameImages);
        }
        let mut schedule = Schedule::new(NextFrameImages);
        schedule.set_executor(SingleThreadedExecutor::new());
        render
            .add_schedule(schedule)
            .init_resource::<FrameImages>()
            .add_systems(ExtractSchedule, extract_ending)
            .add_systems(
                Render,
                (
                    give_back_unused.in_set(RenderSystems::PrepareViews).after(sort_cameras).before(prepare_windows),
                    note_images.in_set(RenderSystems::Render).before(render_system),
                ),
            )
            .add_systems(
                NextFrameImages,
                (release_images.run_if(ending), (prepare_windows, note_taken).chain().run_if(take_next)),
            );
    }
}

pub fn installed(render: &SubApp) -> bool {
    render.world().contains_resource::<FrameImages>()
}

#[derive(Resource, Default)]
struct FrameImages {
    drawn: bool,
    ending: bool,
    taken: Vec<Entity>,
}

fn extract_ending(mut state: ResMut<FrameImages>, mut exits: Extract<MessageReader<AppExit>>) {
    if exits.read().count() > 0 {
        state.ending = true;
    }
}

fn note_images(windows: Query<&ExtractedWindow, With<SurfaceData>>, mut state: ResMut<FrameImages>) {
    state.drawn = !windows.is_empty() && windows.iter().all(|w| w.swap_chain_texture.is_some());
}

fn ending(state: Res<FrameImages>) -> bool {
    state.ending
}

fn take_next(windows: Query<&ExtractedWindow, With<SurfaceData>>, state: Res<FrameImages>) -> bool {
    !state.ending && state.drawn && windows.iter().all(|w| w.swap_chain_texture.is_none())
}

fn note_taken(mut state: ResMut<FrameImages>, windows: Query<(Entity, &ExtractedWindow)>) {
    state.taken.clear();
    state.taken.extend(windows.iter().filter(|(_, w)| w.swap_chain_texture.is_some()).map(|(e, _)| e));
}

fn give_back_unused(
    mut state: ResMut<FrameImages>,
    sorted_cameras: Res<SortedCameras>,
    mut windows: Query<(Entity, MainEntity, &mut ExtractedWindow)>,
) {
    if state.taken.is_empty() {
        return;
    }
    for (entity, main_entity, mut window) in &mut windows {
        if state.taken.contains(&entity)
            && !window.needs_initial_present
            && !camera_writes_to(&sorted_cameras, main_entity)
        {
            release(&mut window);
        }
    }
    state.taken.clear();
}

fn camera_writes_to(cameras: &SortedCameras, window: Entity) -> bool {
    cameras.0.iter().any(|c| {
        matches!(&c.target, Some(NormalizedRenderTarget::Window(w)) if w.entity() == window)
            && matches!(c.output_mode, CameraOutputMode::Write { .. })
    })
}

fn release_images(mut windows: Query<&mut ExtractedWindow>) {
    for mut window in &mut windows {
        release(&mut window);
    }
}

fn release_on_remove(mut world: DeferredWorld, context: HookContext) {
    if let Some(mut window) = world.get_mut::<ExtractedWindow>(context.entity) {
        release(&mut window);
    }
}

fn release(window: &mut ExtractedWindow) {
    window.swap_chain_texture = None;
    window.swap_chain_texture_view = None;
}

#[cfg(test)]
mod tests {
    use bevy::camera::ClearColorConfig;
    use bevy::ecs::schedule::InternedScheduleLabel;
    use bevy::render::camera::SortedCamera;
    use bevy::window::WindowRef;

    use super::*;

    fn entity(index: u32) -> Entity {
        Entity::from_raw_u32(index).expect("an index")
    }

    fn camera(window: Option<Entity>, write: bool) -> SortedCamera {
        SortedCamera {
            entity: Entity::PLACEHOLDER,
            order: 0,
            target: window.and_then(|w| WindowRef::Entity(w).normalize(None)).map(NormalizedRenderTarget::Window),
            output_mode: if write {
                CameraOutputMode::Write { blend_state: None, clear_color: ClearColorConfig::Default }
            } else {
                CameraOutputMode::Skip
            },
        }
    }

    fn render_app(order: Vec<InternedScheduleLabel>) -> App {
        let mut app = App::new();
        let mut render = SubApp::new();
        render.insert_resource(RenderScheduleOrder { labels: order });
        app.insert_sub_app(RenderApp, render);
        app.add_plugins(SwapChainPlugin);
        app
    }

    #[test]
    fn only_a_camera_writing_to_the_window_keeps_its_image() {
        let (window, other) = (entity(1), entity(2));
        let keeps = |cameras: Vec<SortedCamera>| camera_writes_to(&SortedCameras(cameras), window);
        assert!(keeps(vec![camera(Some(window), true)]));
        assert!(keeps(vec![camera(Some(other), true), camera(Some(window), true)]));
        assert!(!keeps(vec![]));
        assert!(!keeps(vec![camera(Some(window), false)]), "a camera that skips its output");
        assert!(!keeps(vec![camera(Some(other), true)]));
        assert!(!keeps(vec![camera(None, true)]), "a camera without a target");
    }

    #[test]
    fn the_schedule_runs_after_render() {
        let mut app = render_app(vec![First.intern(), Render.intern()]);
        let render = app.sub_app_mut(RenderApp);
        assert!(installed(render));
        let labels = &render.world().resource::<RenderScheduleOrder>().labels;
        assert_eq!(labels, &[First.intern(), Render.intern(), NextFrameImages.intern()]);
        assert!(render.world().resource::<Schedules>().contains(NextFrameImages));
        let hooks = render.world_mut().register_component_hooks::<ExtractedWindow>();
        assert!(hooks.try_on_remove(release_on_remove).is_none(), "the image is given back on removal");
    }

    #[test]
    fn without_render_in_the_order_each_frame_takes_its_own() {
        let mut app = render_app(vec![First.intern()]);
        let render = app.sub_app_mut(RenderApp);
        assert!(!installed(render));
        assert_eq!(render.world().resource::<RenderScheduleOrder>().labels, [First.intern()]);
        let hooks = render.world_mut().register_component_hooks::<ExtractedWindow>();
        assert!(hooks.try_on_remove(release_on_remove).is_some(), "no hook set");
    }
}
