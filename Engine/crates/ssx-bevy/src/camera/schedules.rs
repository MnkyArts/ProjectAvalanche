use bevy::core_pipeline::schedule::{Core2d, Core3d};
use bevy::ecs::schedule::{ScheduleLabel, SingleThreadedExecutor};
use bevy::prelude::{App, Plugin};
use bevy::render::RenderApp;

pub struct CameraSchedulesPlugin;

impl Plugin for CameraSchedulesPlugin {
    fn build(&self, _app: &mut App) {}

    fn finish(&self, app: &mut App) {
        let Some(render) = app.get_sub_app_mut(RenderApp) else { return };
        // PORT(choice): the camera schedules run their systems one after another on the render thread instead of on
        // Bevy's multi-threaded executor, which hands each system to the compute pool and waits for it: a view's
        // passes are a short chain of systems that each record into their own command encoder, and the executor's
        // hand-offs cost more than the overlap gains (1080p, `--benchmark`, the dev laptop: the render thread 0.34 /
        // 0.57 ms shorter on Mount Eddie / alps0101, the frame 0.36 / 0.52). The command buffers keep their order:
        // neither schedule has an `ApplyDeferred` (Bevy builds them without automatic sync points), so both executors
        // queue every system's encoder once at the schedule's end in the systems' topological order. Engine
        // mechanics: the original's renderer has no such schedules.
        for label in [Core3d.intern(), Core2d.intern()] {
            if let Some(schedule) = render.get_schedule_mut(label) {
                schedule.set_executor(SingleThreadedExecutor::new());
            }
        }
    }
}
