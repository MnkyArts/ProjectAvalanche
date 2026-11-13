use bevy::ecs::system::SystemParam;
use bevy::input::ButtonState;
use bevy::input::mouse::{MouseButtonInput, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::window::{CursorIcon, CursorMoved, CursorOptions, PrimaryWindow, SystemCursorIcon};
use ssx_ui::avm1::pointer::Cursor;
use ssx_ui::game::pointer::MouseButton as UiButton;
use ssx_ui::geom::RectF;
use ssx_ui::render::{ScaleMode, stage_transform};

use super::GameUiRes;
use crate::ui::{UiCamera, UiScene};

// PORT(choice): a pixel-unit scroll of 40 logical pixels is a wheel notch (a line-unit scroll is one per line); the
// console has no wheel.
const PIXELS_PER_NOTCH: f32 = 40.0;

#[derive(Default)]
pub(super) struct PointerShell {
    shown: bool,
    last: Option<Vec2>,
    scroll: f32,
    applied: Option<(bool, bool)>,
}

#[derive(SystemParam)]
pub(super) struct PointerInput<'w, 's> {
    keys: Res<'w, ButtonInput<KeyCode>>,
    pads: Query<'w, 's, &'static Gamepad>,
    window: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
    moved: MessageReader<'w, 's, CursorMoved>,
    buttons: MessageReader<'w, 's, MouseButtonInput>,
    wheel: MessageReader<'w, 's, MouseWheel>,
    cursor: Query<'w, 's, (Entity, &'static mut CursorOptions), With<PrimaryWindow>>,
    icons: Query<'w, 's, &'static mut CursorIcon, With<PrimaryWindow>>,
    cameras: Query<'w, 's, &'static Camera, With<UiCamera>>,
    scene: Option<ResMut<'w, UiScene>>,
    commands: Commands<'w, 's>,
    state: Local<'s, PointerShell>,
}

pub fn window_to_stage(physical: Vec2, viewport: URect, stage: RectF, mode: ScaleMode) -> Option<(f32, f32)> {
    let size = viewport.size();
    if size.x == 0 || size.y == 0 {
        return None;
    }
    let inv = stage_transform(stage, (size.x, size.y), mode).inverse()?;
    let local = physical - viewport.min.as_vec2();
    let (x, y) = inv.apply(local.x, local.y);
    (x.is_finite() && y.is_finite()).then_some((x / 20.0, y / 20.0))
}

fn on_viewport(point: Vec2, window: Vec2, viewport: URect) -> Vec2 {
    if window.x <= 0.0 || window.y <= 0.0 {
        return point;
    }
    point * viewport.size().as_vec2() / window
}

impl PointerInput<'_, '_> {
    fn stage_point(&self, logical: Vec2, scale: f32) -> Option<(f32, f32)> {
        let scene = self.scene.as_deref()?;
        let viewport = self.cameras.iter().find_map(|c| c.physical_viewport_rect())?;
        let root = scene.player.levels.values().next().and_then(|r| scene.player.get(*r))?;
        let stage = scene.lib.movie(root.movie).stage;
        let window = self.window.single().map_or(logical * scale, |w| w.physical_size().as_vec2());
        window_to_stage(on_viewport(logical * scale, window, viewport), viewport, stage, scene.scale_mode)
    }

    fn script_point(&self, physical: Vec2) -> Option<(f32, f32)> {
        let scene = self.scene.as_deref()?;
        let viewport = self.cameras.iter().find_map(|c| c.physical_viewport_rect())?;
        let root = scene.player.levels.values().next().and_then(|r| scene.player.get(*r))?;
        let stage = scene.lib.movie(root.movie).stage;
        let window = self.window.single().map_or(physical, |w| w.physical_size().as_vec2());
        window_to_stage(on_viewport(physical, window, viewport), viewport, stage, scene.scale_mode)
    }
}

pub(super) fn pointer_system(
    mut p: PointerInput,
    mut shell: ResMut<super::Shell>,
    mut ui: ResMut<GameUiRes>,
    level: Option<Res<crate::shell::game_level::LevelGame>>,
) {
    let riding =
        level.as_ref().is_some_and(|l| l.takes_input()) && !ui.fe.as_ref().is_some_and(|f| f.game_pause.paused);
    let other_input = p.keys.get_just_pressed().len() > 0
        || p.pads.iter().any(|g| g.get_just_pressed().next().is_some() || g.left_stick().length() > 0.5)
        || std::mem::take(&mut shell.host.script_pad);
    if std::mem::take(&mut shell.host.script_pointer) {
        p.state.shown = true;
    }
    let scale = p.window.single().map_or(1.0, |w| w.scale_factor());
    let active = shell.started && !riding;
    for (x, y) in std::mem::take(&mut shell.host.window_moves) {
        match p.script_point(Vec2::new(x, y)) {
            Some((sx, sy)) => {
                info!("shell: pointer at window ({x}, {y}) → stage ({sx:.1}, {sy:.1}) (script)");
                ui.pointer_move(sx, sy);
            }
            None => warn!("shell: pointer at window ({x}, {y}): no UI viewport to map it to (script)"),
        }
    }
    pointer_frame(&mut p, &mut ui, scale, active, other_input);
    for line in std::mem::take(&mut ui.pointer.log) {
        info!("{line}");
    }
}

fn pointer_frame(p: &mut PointerInput, ui: &mut GameUiRes, scale: f32, active: bool, other_input: bool) {
    if other_input {
        p.state.shown = false;
    }
    let moved: Vec<Vec2> = p.moved.read().map(|m| m.position).collect();
    if let Some(&pos) = moved.last()
        && p.state.last != Some(pos)
    {
        p.state.last = Some(pos);
        p.state.shown = true;
        if active && let Some((x, y)) = p.stage_point(pos, scale) {
            ui.pointer_move(x, y);
        }
    }
    let buttons: Vec<(UiButton, bool)> = p
        .buttons
        .read()
        .filter_map(|b| {
            let ui_button = match b.button {
                MouseButton::Left => UiButton::Left,
                MouseButton::Right => UiButton::Right,
                MouseButton::Middle => UiButton::Middle,
                _ => return None,
            };
            Some((ui_button, b.state == ButtonState::Pressed))
        })
        .collect();
    for (b, down) in buttons {
        if down {
            p.state.shown = true;
        }
        if active || !down {
            ui.pointer_button(b, down);
        }
    }
    let mut notches = 0i32;
    for w in p.wheel.read() {
        match w.unit {
            MouseScrollUnit::Line => notches = notches.saturating_add(w.y.round().clamp(-64.0, 64.0) as i32),
            MouseScrollUnit::Pixel => {
                p.state.scroll += w.y.clamp(-4096.0, 4096.0);
                let n = (p.state.scroll / PIXELS_PER_NOTCH).trunc();
                p.state.scroll -= n * PIXELS_PER_NOTCH;
                notches = notches.saturating_add(n as i32);
            }
        }
    }
    if active && notches != 0 {
        ui.pointer_wheel(notches);
    }
    let kind = match p.scene.as_deref_mut().and_then(|s| s.vm.as_mut().map(|vm| (vm, &mut s.lib, &mut s.player))) {
        Some((vm, lib, player)) => vm.pointer_cursor(lib, player),
        None => Cursor::Arrow,
    };
    let visible = p.state.shown && active && kind != Cursor::Hidden;
    let hand = kind == Cursor::Hand;
    if p.state.applied != Some((visible, hand)) {
        p.state.applied = Some((visible, hand));
        info!(
            "shell: the cursor {} ({})",
            if visible { "shown" } else { "hidden" },
            if hand { "hand" } else { "arrow" }
        );
        if let Ok((window, mut options)) = p.cursor.single_mut() {
            options.visible = visible;
            let icon: CursorIcon = if hand { SystemCursorIcon::Pointer } else { SystemCursorIcon::Default }.into();
            match p.icons.single_mut() {
                Ok(mut i) => *i = icon,
                Err(_) => {
                    p.commands.entity(window).insert(icon);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAGE: RectF = RectF { x0: 0.0, y0: 0.0, x1: 25600.0, y1: 14400.0 };

    fn at(w: u32, h: u32, x: f32, y: f32) -> (f32, f32) {
        let vp = URect::new(0, 0, w, h);
        window_to_stage(Vec2::new(x, y), vp, STAGE, ScaleMode::NoBorder).expect("a point")
    }

    fn near(a: (f32, f32), b: (f32, f32)) -> bool {
        (a.0 - b.0).abs() < 0.01 && (a.1 - b.1).abs() < 0.01
    }

    #[test]
    fn window_points_map_onto_the_stage_at_every_size_and_aspect() {
        assert!(near(at(1920, 1080, 960.0, 540.0), (640.0, 360.0)));
        assert!(near(at(1920, 1080, 0.0, 0.0), (0.0, 0.0)));
        assert!(near(at(1920, 1080, 1920.0, 1080.0), (1280.0, 720.0)));
        assert!(near(at(2560, 1440, 1280.0, 720.0), (640.0, 360.0)));
        assert!(near(at(2560, 1440, 256.0, 1296.0), (128.0, 648.0)));
        assert!(near(at(1280, 720, 300.0, 250.0), (300.0, 250.0)));
        assert!(near(at(1024, 768, 512.0, 384.0), (640.0, 360.0)));
        let left = at(1024, 768, 0.0, 0.0);
        assert!(near(left, ((1365.333 - 1024.0) / 2.0 / (768.0 / 720.0), 0.0)), "{left:?}");
        assert!(near(at(1280, 800, 640.0, 400.0), (640.0, 360.0)));
        assert!(near(at(1280, 800, 0.0, 800.0), (64.0, 720.0)));
        assert!(near(at(2560, 1080, 1280.0, 540.0), (640.0, 360.0)));
        assert!(near(at(2560, 1080, 320.0, 0.0), (0.0, 0.0)));
        assert!(near(at(2560, 1080, 2240.0, 1080.0), (1280.0, 720.0)));
        assert!(near(at(2560, 1080, 0.0, 540.0), (-213.333, 360.0)));
        assert!(near(at(3440, 1440, 440.0, 1440.0), (0.0, 720.0)));
        assert!(near(at(5120, 1440, 5120.0, 0.0), (1920.0, 0.0)));
        let vp = URect::new(100, 50, 100 + 1280, 50 + 720);
        let p = window_to_stage(Vec2::new(100.0, 50.0), vp, STAGE, ScaleMode::NoBorder).unwrap();
        assert!(near(p, (0.0, 0.0)));
        assert_eq!(window_to_stage(Vec2::ZERO, URect::new(0, 0, 0, 0), STAGE, ScaleMode::NoBorder), None);
    }

    #[test]
    fn a_render_resolution_scales_the_pointer_with_the_frame() {
        let viewport = URect::new(0, 0, 1920, 1080);
        assert_eq!(
            on_viewport(Vec2::new(3840.0, 2160.0), Vec2::new(3840.0, 2160.0), viewport),
            Vec2::new(1920.0, 1080.0)
        );
        assert_eq!(
            on_viewport(Vec2::new(1920.0, 1080.0), Vec2::new(3840.0, 2160.0), viewport),
            Vec2::new(960.0, 540.0)
        );
        let own = URect::new(0, 0, 3840, 2160);
        assert_eq!(on_viewport(Vec2::new(1920.0, 1080.0), Vec2::new(3840.0, 2160.0), own), Vec2::new(1920.0, 1080.0));
        assert_eq!(on_viewport(Vec2::new(10.0, 20.0), Vec2::ZERO, viewport), Vec2::new(10.0, 20.0));
    }
}
