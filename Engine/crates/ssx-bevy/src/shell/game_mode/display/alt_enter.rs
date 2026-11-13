use std::collections::VecDeque;

use bevy::ecs::message::MessageId;
use bevy::input::ButtonState;
use bevy::input::InputSystems;
use bevy::input::keyboard::{KeyboardFocusLost, KeyboardInput, keyboard_input_system};
use bevy::prelude::*;

const TOGGLES_KEPT: usize = 8;

#[derive(Resource, Default)]
pub struct AltEnter {
    alt: [bool; 2],
    pub pressed: bool,
    toggles: VecDeque<usize>,
}

impl AltEnter {
    pub fn typed<'a>(
        &'a self,
        messages: impl Iterator<Item = (&'a KeyboardInput, MessageId<KeyboardInput>)> + 'a,
    ) -> impl Iterator<Item = &'a KeyboardInput> + 'a {
        messages.filter(|(_, id)| !self.toggles.contains(&id.id)).map(|(m, _)| m)
    }

    // PORT(choice): Alt+Enter toggles the window and fullscreen: the PC convention (DXGI's own default, which wgpu-hal
    // 30's DX12 swap chain turns off with `DXGI_MWA_NO_ALT_ENTER`); Enter alone stays START / A; the 360 has no window.
    pub fn key(&mut self, key: KeyCode, state: ButtonState, repeat: bool) -> bool {
        let down = state == ButtonState::Pressed;
        match key {
            KeyCode::AltLeft => self.alt[0] = down,
            KeyCode::AltRight => self.alt[1] = down,
            KeyCode::Enter | KeyCode::NumpadEnter if down && !repeat && self.alt.contains(&true) => {
                self.pressed = true;
                return true;
            }
            _ => {}
        }
        false
    }

    pub fn focus_lost(&mut self) {
        self.alt = [false; 2];
    }
}

pub fn register(app: &mut App) {
    app.init_resource::<AltEnter>()
        .add_systems(PreUpdate, alt_enter_frame.in_set(InputSystems).after(keyboard_input_system));
}

fn alt_enter_frame(
    mut state: ResMut<AltEnter>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    mut messages: MessageReader<KeyboardInput>,
    mut focus_lost: MessageReader<KeyboardFocusLost>,
) {
    state.pressed = false;
    for (m, id) in messages.read_with_id() {
        if state.key(m.key_code, m.state, m.repeat) {
            if state.toggles.len() == TOGGLES_KEPT {
                state.toggles.pop_front();
            }
            state.toggles.push_back(id.id);
        }
    }
    if focus_lost.read().count() > 0 {
        state.focus_lost();
    }
    if state.pressed {
        for k in [KeyCode::Enter, KeyCode::NumpadEnter] {
            keys.clear_just_pressed(k);
        }
    }
}
