use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::prelude::{ButtonInput, Entity, KeyCode};

use super::controls::PadRead;

pub(super) const KEYS: [KeyCode; 31] = [
    KeyCode::ArrowUp,
    KeyCode::ArrowDown,
    KeyCode::ArrowLeft,
    KeyCode::ArrowRight,
    KeyCode::KeyW,
    KeyCode::KeyA,
    KeyCode::KeyS,
    KeyCode::KeyD,
    KeyCode::Space,
    KeyCode::Enter,
    KeyCode::AltLeft,
    KeyCode::AltRight,
    KeyCode::Backspace,
    KeyCode::KeyC,
    KeyCode::Numpad8,
    KeyCode::Numpad4,
    KeyCode::Numpad6,
    KeyCode::Numpad2,
    KeyCode::Numpad5,
    KeyCode::KeyI,
    KeyCode::KeyJ,
    KeyCode::KeyK,
    KeyCode::KeyL,
    KeyCode::KeyQ,
    KeyCode::KeyE,
    KeyCode::KeyZ,
    KeyCode::KeyX,
    KeyCode::ShiftLeft,
    KeyCode::ShiftRight,
    KeyCode::ControlLeft,
    KeyCode::ControlRight,
];

pub(super) const BUTTONS: [GamepadButton; 14] = [
    GamepadButton::South,
    GamepadButton::East,
    GamepadButton::North,
    GamepadButton::West,
    GamepadButton::LeftTrigger,
    GamepadButton::RightTrigger,
    GamepadButton::LeftTrigger2,
    GamepadButton::RightTrigger2,
    GamepadButton::Select,
    GamepadButton::RightThumb,
    GamepadButton::DPadUp,
    GamepadButton::DPadDown,
    GamepadButton::DPadLeft,
    GamepadButton::DPadRight,
];

const MAX_PENDING: u8 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Flips {
    read: bool,
    pending: u8,
}

impl Flips {
    fn latest(self) -> bool {
        self.read != (self.pending % 2 == 1)
    }

    fn observe(&mut self, now: bool, pressed: bool, released: bool) {
        let flips = if self.latest() != now {
            1
        } else if pressed && released {
            2
        } else {
            0
        };
        self.pending += flips;
        while self.pending > MAX_PENDING {
            self.pending -= 2;
        }
    }

    fn step(&mut self) -> bool {
        if self.pending > 0 {
            self.read = !self.read;
            self.pending -= 1;
        }
        self.read
    }

    fn flush(&mut self, now: bool) {
        *self = Flips { read: now, pending: 0 };
    }
}

type PadFlips = [Flips; BUTTONS.len()];

fn button_index(b: GamepadButton) -> Option<usize> {
    BUTTONS.iter().position(|&x| x == b)
}

// PORT(choice): every press and release of the ride's keys and pad buttons reaches a step: each frame's input adds
// the flips Bevy's state shows (the button changed since the last look; a press and a release in one frame from its
// `just_pressed` and `just_released`), and each step reads the next pending flip of each button, else its state — a
// tap released before any step ran is held for one step and released at the next; flips beyond two taps are dropped
// in pairs. The console has no events: each 30 Hz step polls the pads once (`XamInputGetState` through 0x8317ACD8,
// 0x82BD6CF0 at 0x823D2FE8 in `FSGameSimulationBE_Step`), on its own thread at a period of wall time (0x83047120,
// 0x83046C48; at most one catch-up step, 0x83046CE8), so it sees a press held at a poll; the port's steps run in
// bursts at the frames' starts and its events come once a frame, so a poll's moment does not exist there. The sticks
// and the triggers' travel are read as the frame left them (no flips: the step takes the newest value, as a poll does).
#[derive(Debug, Default)]
pub(super) struct PadLatch {
    keys: [Flips; KEYS.len()],
    pads: Vec<(Entity, PadFlips)>,
    view: ButtonInput<KeyCode>,
}

impl PadLatch {
    pub fn frame<'a>(&mut self, keys: &ButtonInput<KeyCode>, pads: impl Iterator<Item = (Entity, &'a Gamepad)>) {
        for (f, &k) in self.keys.iter_mut().zip(&KEYS) {
            f.observe(keys.pressed(k), keys.just_pressed(k), keys.just_released(k));
        }
        self.sync_pads(pads, |f, pad, b| f.observe(pad.pressed(b), pad.just_pressed(b), pad.just_released(b)));
    }

    pub fn step<'a>(&mut self, keys: &ButtonInput<KeyCode>, pads: impl Iterator<Item = (Entity, &'a Gamepad)>) {
        self.view.release_all();
        for k in keys.get_pressed() {
            self.view.press(*k);
        }
        for (f, &k) in self.keys.iter_mut().zip(&KEYS) {
            f.observe(keys.pressed(k), false, false);
            if f.step() {
                self.view.press(k);
            } else {
                self.view.release(k);
            }
        }
        self.view.clear();
        self.sync_pads(pads, |f, pad, b| {
            f.observe(pad.pressed(b), false, false);
            f.step();
        });
    }

    pub fn flush<'a>(&mut self, keys: &ButtonInput<KeyCode>, pads: impl Iterator<Item = (Entity, &'a Gamepad)>) {
        for (f, &k) in self.keys.iter_mut().zip(&KEYS) {
            f.flush(keys.pressed(k));
        }
        self.sync_pads(pads, |f, pad, b| f.flush(pad.pressed(b)));
    }

    fn sync_pads<'a>(
        &mut self,
        pads: impl Iterator<Item = (Entity, &'a Gamepad)>,
        mut look: impl FnMut(&mut Flips, &Gamepad, GamepadButton),
    ) {
        let mut kept = 0;
        for (entity, pad) in pads {
            let i = match self.pads.iter().position(|(e, _)| *e == entity) {
                Some(i) => i,
                None => {
                    let mut flips = PadFlips::default();
                    for (f, &b) in flips.iter_mut().zip(&BUTTONS) {
                        f.flush(pad.pressed(b));
                    }
                    self.pads.push((entity, flips));
                    self.pads.len() - 1
                }
            };
            self.pads.swap(kept, i);
            for (f, &b) in self.pads[kept].1.iter_mut().zip(&BUTTONS) {
                look(f, pad, b);
            }
            kept += 1;
        }
        self.pads.truncate(kept);
    }

    pub fn keys(&self) -> &ButtonInput<KeyCode> {
        &self.view
    }

    pub fn pad<'a>(&'a self, entity: Entity, pad: &'a Gamepad) -> StepPad<'a> {
        StepPad { pad, buttons: self.pads.iter().find(|(e, _)| *e == entity).map(|(_, f)| f) }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct StepPad<'a> {
    pad: &'a Gamepad,
    buttons: Option<&'a PadFlips>,
}

impl PadRead for StepPad<'_> {
    fn pressed(&self, button: GamepadButton) -> bool {
        match (self.buttons, button_index(button)) {
            (Some(f), Some(i)) => f[i].read,
            _ => self.pad.pressed(button),
        }
    }

    fn value(&self, button: GamepadButton) -> f32 {
        self.pad.get(button).unwrap_or(0.0)
    }

    fn left_stick(&self) -> bevy::math::Vec2 {
        self.pad.left_stick()
    }

    fn right_stick(&self) -> bevy::math::Vec2 {
        self.pad.right_stick()
    }
}

#[cfg(test)]
mod tests;
