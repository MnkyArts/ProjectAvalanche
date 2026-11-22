use bevy::app::MainScheduleOrder;
use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;
use bevy::render::error_handler::{ErrorType, RenderError, RenderErrorHandler, RenderErrorPolicy};
use bevy::render::render_resource::PollType;
use bevy::render::renderer::{RenderAdapterInfo, RenderDevice, RenderGraph, RenderGraphSystems};
use bevy::render::view::prepare_windows;
use bevy::render::{Extract, ExtractSchedule, Render, RenderApp, RenderSystems};

use crate::render::swap_chain::NextFrameImages;
use crate::render::terrain::TrackStats;
use crate::ride::Ride;
use crate::shell::game_level::LevelGame;

#[derive(Resource, Clone, Copy, Debug)]
pub struct RenderStopped {
    pub device_lost: bool,
}

#[derive(Resource, Default)]
pub struct ShellEndsOnRenderStop {
    pub exit_written: bool,
}

#[derive(Resource)]
struct LoseAt {
    at: f32,
    point: LosePoint,
    due: Option<f32>,
    done: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LosePoint {
    Acquire,
    Prepare,
    Submit,
}

impl LosePoint {
    fn name(self) -> &'static str {
        match self {
            LosePoint::Acquire => "before the swap-chain acquire",
            LosePoint::Prepare => "after the frame's acquire, before its prepare systems",
            LosePoint::Submit => "after the frame's submit",
        }
    }
}

fn parse_lose_at(v: &str) -> Option<(f32, LosePoint)> {
    let (at, point) = v.split_once(',').unwrap_or((v, "submit"));
    let at = at.trim().parse::<f32>().ok().filter(|s| s.is_finite())?;
    let point = match point.trim() {
        "acquire" => LosePoint::Acquire,
        "prepare" => LosePoint::Prepare,
        "submit" => LosePoint::Submit,
        _ => return None,
    };
    Some((at, point))
}

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
struct AfterLast;

pub fn plugin(app: &mut App) {
    app.insert_resource(RenderErrorHandler(on_render_error)).add_systems(First, end_without_shell);
    app.world_mut().resource_mut::<MainScheduleOrder>().insert_after(Last, AfterLast);
    app.add_systems(AfterLast, end_after_lost_device);
    // PORT(choice): `SSX_GPU_LOSE_AT=<s>[,acquire|prepare|submit]` (a test switch) destroys the graphics device once a
    // ride has run s seconds, at that point of the render frame (after the submit when none is named)
    // (`wgpu::Device::destroy`, WebGPU's `GPUDevice.destroy()`: the device turns invalid at once and every later map,
    // copy, submission and present fails) and waits for its queue to be idle, where wgpu answers the device's lost
    // callback (`Destroyed`; a driver's loss answers it at once, `Unknown`), so the end of a lost device can be run on
    // a machine whose driver never loses one. The console had no such switch.
    let Ok(value) = std::env::var("SSX_GPU_LOSE_AT") else { return };
    let Some((at, point)) = parse_lose_at(&value) else {
        warn!("SSX_GPU_LOSE_AT={value:?}: not <seconds>[,acquire|prepare|submit]; the switch stays off");
        return;
    };
    let Some(render) = app.get_sub_app_mut(RenderApp) else { return };
    render.insert_resource(LoseAt { at, point, due: None, done: false }).add_systems(ExtractSchedule, extract_lose_at);
    match point {
        LosePoint::Acquire if crate::render::swap_chain::installed(render) => {
            render.add_systems(NextFrameImages, lose_device.before(prepare_windows));
        }
        LosePoint::Acquire => {
            render.add_systems(Render, lose_device.in_set(RenderSystems::PrepareViews).before(prepare_windows));
        }
        LosePoint::Prepare => {
            render.add_systems(Render, lose_device.after(RenderSystems::PrepareViews).before(RenderSystems::Queue));
        }
        LosePoint::Submit => {
            render.add_systems(
                RenderGraph,
                lose_device.after(RenderGraphSystems::Submit).before(RenderGraphSystems::Finish),
            );
        }
    }
}

fn extract_lose_at(mut lose: ResMut<LoseAt>, ride: Extract<Option<Res<Ride>>>) {
    lose.due = ride.as_ref().filter(|r| !r.paused && !r.hold && r.render_time >= lose.at).map(|r| r.render_time);
}

fn lose_device(mut lose: ResMut<LoseAt>, device: Res<RenderDevice>) {
    let Some(time) = lose.due else { return };
    if lose.done {
        return;
    }
    lose.done = true;
    warn!(
        "SSX_GPU_LOSE_AT: the graphics device is destroyed {} at ride time {time:.2} s (a test switch)",
        lose.point.name()
    );
    device.wgpu_device().destroy();
    match device.poll(PollType::wait_indefinitely()) {
        Ok(status) => info!("SSX_GPU_LOSE_AT: the destroyed device's queue is idle ({status:?})"),
        Err(e) => warn!("SSX_GPU_LOSE_AT: waiting for the destroyed device's queue failed: {e}"),
    }
}

fn on_render_error(error: &RenderError, main: &mut World, _render: &mut World) -> RenderErrorPolicy {
    if main.contains_resource::<RenderStopped>() {
        return RenderErrorPolicy::StopRendering;
    }
    let device_lost = error.ty == ErrorType::DeviceLost;
    let shell = main.contains_resource::<ShellEndsOnRenderStop>();
    let gpu = main.get_resource::<RenderAdapterInfo>().map_or_else(|| "the graphics adapter".to_string(), adapter);
    error!("{}", report(error.ty, &error.description, &gpu, &place(main), shell));
    main.insert_resource(RenderStopped { device_lost });
    // PORT(choice): the renderer is not recovered: rendering stops and the app ends (the game after its writers have
    // written what was asked of them). Bevy 0.20's `RenderErrorPolicy::Recover` makes a new device and extracts again
    // only the render assets the main world still holds; 15 places of the port make images and a mesh for the render
    // world alone (`RenderAssetUsages::RENDER_WORLD`: the terrain's texture arrays, the snow tracks, the sky, the
    // post-FX colour cube, the local lights' projections …), which the main world drops once they are extracted, so a
    // recovered device would draw the level without them; and wgpu 30 keeps a lost device's acquired swap-chain image
    // (`Surface::present` / `discard` return early on an invalid device), which the surfaces' reconfiguration needs
    // back. The console had no lost device to answer.
    RenderErrorPolicy::StopRendering
}

fn end_without_shell(
    stopped: Option<Res<RenderStopped>>,
    shell: Option<Res<ShellEndsOnRenderStop>>,
    mut exit: MessageWriter<AppExit>,
) {
    if stopped.is_some_and(|s| s.is_added()) && shell.is_none() {
        exit.write(AppExit::error());
    }
}

fn end_after_lost_device(
    stopped: Option<Res<RenderStopped>>,
    shell: Option<Res<ShellEndsOnRenderStop>>,
    mut exits: MessageReader<AppExit>,
) {
    let Some(code) = exit_code(exits.read()) else { return };
    if !stopped.is_some_and(|s| s.device_lost) || shell.is_some_and(|s| !s.exit_written) {
        return;
    }
    // PORT(choice): after a lost device the process ends here (`std::process::exit`, the log flushed first, the code
    // the app asked for), without the app's teardown: wgpu 30.0.2 keeps a lost device's acquired swap-chain image
    // (wgpu-core `Surface::present` / `discard` return early on an invalid device; only `Device::maintain`'s release
    // of a destroyed device's resources gives it back, which a driver's loss never reaches: its fence wait fails
    // first), and the render world's surface drop unconfigures the swap chain: wgpu-hal's Vulkan
    // `NativeSwapchain::release_resources` then panics ("Trying to destroy a SwapchainAcquireSemaphore that is still
    // in use by a SurfaceTexture", the tester's log after the profile was saved). What the teardown would end is done
    // by then (the profile, ghosts, settings and window state written, the script host stopped) or ends with the
    // process (threads, the audio device, the window). The console had no lost device to answer.
    error!("renderer: the graphics device was lost: the process ends with exit code {code}, without the teardown");
    ssx_core::output::flush();
    std::process::exit(code);
}

fn exit_code<'a>(exits: impl Iterator<Item = &'a AppExit>) -> Option<i32> {
    let mut any = false;
    for exit in exits {
        if let AppExit::Error(code) = exit {
            return Some(i32::from(code.get()));
        }
        any = true;
    }
    any.then_some(0)
}

fn adapter(info: &RenderAdapterInfo) -> String {
    format!(
        "{:?} ({:?}, {:?}, driver {:?} {:?})",
        info.name, info.backend, info.device_type, info.driver, info.driver_info
    )
}

fn place(world: &World) -> String {
    let level = world
        .get_resource::<LevelGame>()
        .map(LevelGame::place)
        .or_else(|| world.get_resource::<TrackStats>().map(|t| t.name.clone()));
    let ride = world.get_resource::<Ride>().map(|r| {
        let p = r.render.traj;
        format!("ride time {:.1} s, the rider at [{:.0}, {:.0}, {:.0}]", r.render_time, p.x, p.y, p.z)
    });
    match (level, ride) {
        (Some(level), Some(ride)) => format!("in {level} at {ride}"),
        (Some(level), None) => format!("in {level}"),
        (None, _) => "outside a level".to_string(),
    }
}

fn report(ty: ErrorType, description: &str, gpu: &str, place: &str, shell: bool) -> String {
    let end = if shell { "the game ends once what it was saving is written" } else { "the app ends" };
    let said = if description.is_empty() { String::new() } else { format!(" ({description:?})") };
    match ty {
        ErrorType::DeviceLost => format!(
            "renderer: the graphics device was lost{said} on {gpu}, {place}; nothing more is drawn and \
             {end}. The graphics driver gave the GPU up: it stopped answering (Windows resets a GPU that has not \
             answered for 2 s), its work faulted, or it was removed. Update the graphics driver; WGPU_BACKEND=dx12 or \
             =vulkan chooses the other backend"
        ),
        other => {
            format!("renderer: a {other:?} error{said} on {gpu}, {place}; nothing more is drawn and {end}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::message::Messages;

    fn lost() -> RenderError {
        RenderError { ty: ErrorType::DeviceLost, description: "Unknown Device is lost".into(), source: None }
    }

    fn exits(world: &World) -> usize {
        world.resource::<Messages<AppExit>>().iter_current_update_messages().count()
    }

    #[test]
    fn a_lost_device_without_a_shell_ends_the_app_once_in_the_next_frame() {
        let (mut main, mut render) = (World::new(), World::new());
        main.init_resource::<Messages<AppExit>>();
        let end = main.register_system(end_without_shell);
        for _ in 0..3 {
            assert!(matches!(on_render_error(&lost(), &mut main, &mut render), RenderErrorPolicy::StopRendering));
        }
        assert_eq!(exits(&main), 0);
        assert!(main.resource::<RenderStopped>().device_lost);
        for _ in 0..3 {
            main.run_system(end).expect("the end runs");
        }
        assert_eq!(exits(&main), 1);
    }

    #[test]
    fn a_lost_device_in_the_game_leaves_the_exit_to_the_shell() {
        let (mut main, mut render) = (World::new(), World::new());
        main.init_resource::<Messages<AppExit>>();
        main.insert_resource(ShellEndsOnRenderStop::default());
        let end = main.register_system(end_without_shell);
        assert!(matches!(on_render_error(&lost(), &mut main, &mut render), RenderErrorPolicy::StopRendering));
        main.run_system(end).expect("the end runs");
        assert_eq!(exits(&main), 0);
        assert!(main.resource::<RenderStopped>().device_lost);
    }

    #[test]
    fn the_process_ends_with_the_code_the_app_would_end_with() {
        let error = |n: u8| AppExit::Error(std::num::NonZero::new(n).expect("non-zero"));
        assert_eq!(exit_code([].iter()), None);
        assert_eq!(exit_code([AppExit::Success].iter()), Some(0));
        assert_eq!(exit_code([AppExit::Success, error(1)].iter()), Some(1));
        assert_eq!(exit_code([error(3), error(1), AppExit::Success].iter()), Some(3));
    }

    #[test]
    fn the_switch_reads_its_second_and_point() {
        assert_eq!(parse_lose_at("3"), Some((3.0, LosePoint::Submit)));
        assert_eq!(parse_lose_at("2.5,acquire"), Some((2.5, LosePoint::Acquire)));
        assert_eq!(parse_lose_at(" 4 , prepare "), Some((4.0, LosePoint::Prepare)));
        assert_eq!(parse_lose_at("1,submit"), Some((1.0, LosePoint::Submit)));
        assert_eq!(parse_lose_at("1,present"), None);
        assert_eq!(parse_lose_at("NaN"), None);
        assert_eq!(parse_lose_at("inf,acquire"), None);
        assert_eq!(parse_lose_at(""), None);
    }

    #[test]
    fn the_line_names_the_gpu_and_the_place() {
        let line = report(ErrorType::DeviceLost, "Device is lost", "\"GTX\"", "in x on y", true);
        assert!(
            line.starts_with("renderer: the graphics device was lost (\"Device is lost\") on \"GTX\", in x on y;"),
            "{line}"
        );
        let destroyed = report(ErrorType::DeviceLost, "", "g", "outside a level", true);
        assert!(destroyed.starts_with("renderer: the graphics device was lost on g, outside a level;"), "{destroyed}");
        assert!(line.contains("once what it was saving is written"), "{line}");
        let other = report(ErrorType::Validation, "bad", "g", "outside a level", false);
        assert!(other.starts_with("renderer: a Validation error"), "{other}");
        assert!(other.ends_with("the app ends"), "{other}");
    }
}
