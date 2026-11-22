use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bevy::prelude::*;
use bevy::render::render_resource::PollType;
use bevy::render::renderer::{RenderDevice, render_system};
use bevy::render::{Render, RenderApp, RenderStartup, RenderSystems};

// PORT(choice): a thread polls the device every 500 µs from a frame's submission until the GPU has finished all
// submitted work, then sleeps until the next submission. Before, the finished submissions were cleaned up only inside
// the render thread's next `Queue::submit` (wgpu 30.0.2 `Queue::submit` → `Device::maintain(Poll)`): dropping the
// trackers of about 24 finished command buffers a frame, 1.22 ms of its 2.89 ms on Mount Eddie (traced with wgpu's
// spans). The same commands in the same submissions. A buffer map is now answered on this thread within 0.5 ms of the
// GPU finishing its copy instead of at the next submit; the player shadow query's value (`particles/shadow_query.rs`)
// reaches the main world at the first extract after that, as the console reads its query back once the draw's fence
// has passed (0x82FBB620). wgpu's device and its clean-up are the port's renderer; the console had neither.
const POLL_EVERY: Duration = Duration::from_micros(500);

pub struct DevicePollPlugin;

impl Plugin for DevicePollPlugin {
    fn build(&self, app: &mut App) {
        let Some(render) = app.get_sub_app_mut(RenderApp) else { return };
        render
            .add_systems(RenderStartup, start)
            .add_systems(Render, wake.after(render_system).in_set(RenderSystems::Render));
    }
}

#[derive(Default)]
struct State {
    submitted: bool,
    stop: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Resource)]
struct DevicePoll(Arc<Shared>);

impl Drop for DevicePoll {
    fn drop(&mut self) {
        self.0.lock().stop = true;
        self.0.wake.notify_one();
    }
}

fn start(mut commands: Commands, device: Res<RenderDevice>) {
    let shared = Arc::new(Shared::default());
    let (thread, device) = (shared.clone(), device.clone());
    match std::thread::Builder::new().name("ssx device poll".into()).spawn(move || run(&thread, &device)) {
        Ok(_) => commands.insert_resource(DevicePoll(shared)),
        Err(e) => warn!("render: no device poll thread ({e}); the finished submissions are cleaned up at each submit"),
    }
}

fn run(shared: &Shared, device: &RenderDevice) {
    loop {
        {
            let mut state = shared.lock();
            while !state.submitted && !state.stop {
                state = shared.wake.wait(state).unwrap_or_else(PoisonError::into_inner);
            }
            if state.stop {
                return;
            }
            state.submitted = false;
        }
        loop {
            std::thread::sleep(POLL_EVERY);
            if shared.lock().stop {
                return;
            }
            match device.poll(PollType::Poll) {
                Ok(wgpu::PollStatus::QueueEmpty) => break,
                Ok(_) => {}
                Err(e) => {
                    info!("render: the device poll thread ends: {e}");
                    return;
                }
            }
        }
    }
}

fn wake(poll: Option<Res<DevicePoll>>) {
    let Some(poll) = poll else { return };
    poll.0.lock().submitted = true;
    poll.0.wake.notify_one();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noop_device() -> (RenderDevice, wgpu::Queue) {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::NOOP,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            display: None,
            backend_options: wgpu::BackendOptions {
                noop: wgpu::NoopBackendOptions { enable: true, ..Default::default() },
                ..Default::default()
            },
        });
        let adapter = bevy::tasks::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("noop adapter");
        let (device, queue) =
            bevy::tasks::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).expect("noop device");
        (RenderDevice::from(device), queue)
    }

    #[test]
    fn the_thread_cleans_up_a_submission_and_ends_with_the_render_world() {
        let (device, queue) = noop_device();
        let shared = Arc::new(Shared::default());
        let (thread, polled) = (shared.clone(), device.clone());
        let handle = std::thread::spawn(move || run(&thread, &polled));
        let encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("test") });
        queue.submit([encoder.finish()]);
        let poll = DevicePoll(shared.clone());
        poll.0.lock().submitted = true;
        poll.0.wake.notify_one();
        while shared.lock().submitted {
            std::thread::sleep(POLL_EVERY);
        }
        drop(poll);
        assert!(handle.join().is_ok(), "the thread ends without a panic");
        assert!(shared.lock().stop);
    }
}
