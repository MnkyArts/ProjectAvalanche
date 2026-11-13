use std::collections::HashMap;

use crate::avm1::Ctx;
use crate::avm1::value::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Start,
    DigitalOne,
    DigitalTwo,
    DigitalThree,
    DigitalFour,
    LeftTrigger,
    RightTrigger,
    LeftBumper,
    RightBumper,
    Select,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    Disconnect,
    Ras,
}

impl Button {
    pub fn code_name(self) -> &'static str {
        match self {
            Button::Start => "START",
            Button::DigitalOne => "DIGITAL_ONE",
            Button::DigitalTwo => "DIGITAL_TWO",
            Button::DigitalThree => "DIGITAL_THREE",
            Button::DigitalFour => "DIGITAL_FOUR",
            Button::LeftTrigger => "LEFT_TRIGGER",
            Button::RightTrigger => "RIGHT_TRIGGER",
            Button::LeftBumper => "LEFT_BUMPER",
            Button::RightBumper => "RIGHT_BUMPER",
            Button::Select => "SELECT",
            Button::DpadUp => "DPAD_UP",
            Button::DpadDown => "DPAD_DOWN",
            Button::DpadLeft => "DPAD_LEFT",
            Button::DpadRight => "DPAD_RIGHT",
            Button::Disconnect => "DISCONNECT",
            Button::Ras => "RAS",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    Button { button: Button, down: bool },
}

pub const PADS: usize = 4;

pub const SCRIPT_DEVICE: u8 = 0;

// PORT(choice): the PC keyboard is device 4, the first keyboard after the four pads (0x831789E8; the UI manager's
// keyboard handler 0x82576E58 is not read); the player's keyboard is the port's, the console reads pads.
pub const KEYBOARD_DEVICE: u8 = 4;

#[derive(Clone, Debug, Default)]
pub struct InputProvider {
    pub subscribers: Vec<String>,
    queue: Vec<(Input, u8)>,
    last: HashMap<(u8, Button), bool>,
    deferred: HashMap<(u8, Button), bool>,
}

impl InputProvider {
    pub fn subscribe(&mut self, path: &str) {
        if !self.subscribers.iter().any(|p| p == path) {
            self.subscribers.push(path.to_string());
        }
    }

    pub fn unsubscribe(&mut self, path: &str) {
        self.subscribers.retain(|p| p != path);
    }

    pub fn push(&mut self, i: Input) {
        self.queue.push((i, SCRIPT_DEVICE));
    }

    pub fn push_from(&mut self, i: Input, device: u8) {
        self.queue.push((i, device));
    }

    pub fn clear(&mut self) {
        self.queue.clear();
    }

    pub fn defer(&mut self) {
        for &(Input::Button { button, down }, device) in &self.queue {
            self.deferred.insert((device, button), down);
        }
        self.queue.clear();
    }

    pub fn resume(&mut self) {
        let mut now = std::mem::take(&mut self.deferred);
        if now.is_empty() {
            return;
        }
        let mut rest = Vec::with_capacity(self.queue.len());
        for (input, device) in std::mem::take(&mut self.queue) {
            let Input::Button { button, down } = input;
            match now.get_mut(&(device, button)) {
                Some(state) => *state = down,
                None => rest.push((input, device)),
            }
        }
        let mut changed: Vec<((u8, Button), bool)> =
            now.into_iter().filter(|(key, down)| self.last.get(key).copied().unwrap_or(false) != *down).collect();
        changed.sort_by_key(|&((device, button), _)| (device, button as u8));
        self.queue =
            changed.into_iter().map(|((device, button), down)| (Input::Button { button, down }, device)).collect();
        self.queue.extend(rest);
    }

    pub fn deliver(&mut self, cx: &mut Ctx) -> usize {
        let mut n = 0;
        for (i, device) in std::mem::take(&mut self.queue) {
            let Input::Button { button, down } = i;
            self.last.insert((device, button), down);
            let obj = Value::Object(cx.new_object());
            let constant = |cx: &mut Ctx, class: &str, member: &str| -> Value {
                let g = Value::Object(cx.vm.global);
                let ux = cx.get_member(&g, "ux").unwrap_or_default();
                let input = cx.get_member(&ux, "input").unwrap_or_default();
                let c = cx.get_member(&input, class).unwrap_or_default();
                let v = cx.get_member(&c, member).unwrap_or_default();
                if !v.is_nullish() {
                    return v;
                }
                let proto = cx.get_member(&c, "prototype").unwrap_or_default();
                cx.get_member(&proto, member).unwrap_or_default()
            };
            let ty = constant(cx, "InputManager", "INPUT_TYPE_DIGTIAL");
            let code = constant(cx, "InputCode", button.code_name());
            let state = constant(cx, "InputManager", if down { "KEY_STATE_DOWN" } else { "KEY_STATE_UP" });
            let device = Value::Number(f64::from(device));
            for (k, v) in [("type", ty), ("keyCode", code), ("keyState", state), ("deviceId", device)] {
                let _ = cx.set_member(&obj, k, v);
            }
            for p in self.subscribers.clone() {
                if super::invoke_path(cx, &format!("{p}.ProcessInput"), std::slice::from_ref(&obj)).is_some() {
                    n += 1;
                }
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(p: &mut InputProvider, b: Button, down: bool) {
        p.push(Input::Button { button: b, down });
    }

    #[test]
    fn a_button_that_changes_while_the_reader_is_locked_is_read_after_it() {
        let mut p = InputProvider::default();
        press(&mut p, Button::DigitalOne, true);
        p.defer();
        p.resume();
        assert_eq!(p.queue, vec![(Input::Button { button: Button::DigitalOne, down: true }, 0)]);

        let mut p = InputProvider::default();
        press(&mut p, Button::DigitalOne, true);
        p.defer();
        press(&mut p, Button::DigitalOne, false);
        p.defer();
        p.resume();
        assert!(p.queue.is_empty(), "a press and release inside the wait cancels");

        let mut p = InputProvider::default();
        p.last.insert((0, Button::DigitalOne), true);
        press(&mut p, Button::DigitalOne, false);
        p.defer();
        p.resume();
        assert_eq!(p.queue, vec![(Input::Button { button: Button::DigitalOne, down: false }, 0)]);

        let mut p = InputProvider::default();
        p.last.insert((0, Button::DigitalOne), true);
        press(&mut p, Button::DigitalOne, false);
        p.defer();
        press(&mut p, Button::DigitalOne, true);
        p.defer();
        p.resume();
        assert!(p.queue.is_empty(), "held before and after the wait: no change");
    }

    #[test]
    fn a_button_released_in_the_wait_and_pressed_before_the_next_read_stays_held() {
        let a = |down| Input::Button { button: Button::DigitalOne, down };
        let b = |down| Input::Button { button: Button::DigitalTwo, down };
        let x = |down| Input::Button { button: Button::DigitalThree, down };
        let mut p = InputProvider::default();
        p.last.insert((0, Button::DigitalOne), true);
        press(&mut p, Button::DigitalOne, false);
        press(&mut p, Button::DigitalThree, true);
        p.defer();
        press(&mut p, Button::DigitalTwo, true);
        press(&mut p, Button::DigitalOne, true);
        p.resume();
        assert_eq!(p.queue, vec![(x(true), 0), (b(true), 0)], "X pressed in the wait first, then this update's B");
        assert!(!p.queue.contains(&(a(false), 0)) && !p.queue.contains(&(a(true), 0)));
    }
}
