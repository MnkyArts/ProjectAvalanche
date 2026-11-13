use std::time::Duration;

use bevy::prelude::*;
use bevy::winit::{UpdateMode, WinitSettings};

use crate::ui;

const HIDDEN_UPDATES: f64 = 20.0;

#[derive(Resource, Default)]
pub struct WindowHidden {
    pub hidden: bool,
    occluded: bool,
    saved: Option<WinitSettings>,
    test: Option<(f64, f64)>,
}

impl WindowHidden {
    pub(super) fn from_env() -> Self {
        let test = std::env::var("SSX_SHELL_HIDDEN").ok().and_then(|v| {
            let (a, b) = v.split_once(':')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        });
        WindowHidden { test, ..default() }
    }
}

// PORT(choice): a minimised or occluded window updates at most `HIDDEN_UPDATES` times a second and builds no UI render
// list (hidden: winit's `is_minimized`, a 0 × 0 size or `WindowOccluded`; measured minimised: 81–83 % of a core against
// 172 % before). Wayland reports none of them: winit 0.30.13 gets xdg_toplevel's `suspended` state from SCTK 0.19.2 but
// only uses it to spare a resize, answers `is_minimized` with None and emits no `Occluded`; its frame-callback pacing
// (`pre_present_notify`, which would leave a hidden window without redraws) is not called by Bevy 0.20.0 (nor 0.19.1)
// and would pace every unfocused frame too; a winit that reports `suspended` for Wayland would cover it there; the
// console has no window.
#[allow(clippy::too_many_arguments)]
pub fn hidden_frame(
    _main_thread: bevy::ecs::system::NonSendMarker,
    windows: Query<(Entity, &Window), With<bevy::window::PrimaryWindow>>,
    mut occluded: MessageReader<bevy::window::WindowOccluded>,
    mut state: ResMut<WindowHidden>,
    mut settings: ResMut<WinitSettings>,
    mut scene: ResMut<ui::UiScene>,
    time: Res<Time<Real>>,
) {
    if let Some(ev) = occluded.read().last() {
        state.occluded = ev.occluded;
    }
    let Ok((entity, window)) = windows.single() else { return };
    let zero_size = window.physical_width() == 0 || window.physical_height() == 0;
    let minimised = !window.focused
        && bevy::winit::WINIT_WINDOWS
            .with_borrow(|w| w.get_window(entity).and_then(|w| w.is_minimized()))
            .unwrap_or(false);
    let test = state.test.is_some_and(|(a, b)| (a..b).contains(&time.elapsed_secs_f64()));
    let hidden = zero_size || minimised || state.occluded || test;
    if hidden == state.hidden {
        return;
    }
    state.hidden = hidden;
    scene.window_hidden = hidden;
    if hidden {
        let wait = Duration::from_secs_f64(1.0 / HIDDEN_UPDATES);
        let low = UpdateMode::reactive_low_power(wait);
        state.saved = Some(std::mem::replace(&mut *settings, WinitSettings { focused_mode: low, unfocused_mode: low }));
        info!(
            "shell: the window is hidden (minimised {minimised}, 0 × 0 {zero_size}, occluded {}, test {test}): at most \
             {HIDDEN_UPDATES} updates a second, no UI render list",
            state.occluded
        );
    } else {
        if let Some(saved) = state.saved.take() {
            *settings = saved;
        }
        info!("shell: the window shows again: the update modes as before");
    }
}
