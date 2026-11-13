// Project Avalanche patch T23 (Engine/vendor/PATCHES.md): SDL's automatic gamepad mapping for a Linux evdev device
// that no mapping row names, translated into Rust from SDL 3.4.20 (tag `release-3.4.20`, commit
// f00dc919ca35b9756316e86c4777392de442f9b2):
//   - `LINUX_JoystickGetGamepadMapping`, src/joystick/linux/SDL_sysjoystick.c:2287-2820 (the rule);
//   - `ConfigJoystick`, 1225-1345, and `GuessIfAxesAreDigitalHat`, 1192-1223 (which keys, axes and hats SDL sees);
//   - `SDL_PrivateGenerateAutomaticGamepadMapping`, src/joystick/SDL_gamepad.c:2306-2366 (the row it writes).
// This is an altered version of that code (a translation; SDL is not otherwise included). SDL's notice:
//
//   Simple DirectMedia Layer
//   Copyright (C) 1997-2026 Sam Lantinga <slouken@libsdl.org>
//
//   This software is provided 'as-is', without any express or implied
//   warranty.  In no event will the authors be held liable for any damages
//   arising from the use of this software.
//
//   Permission is granted to anyone to use this software for any purpose,
//   including commercial applications, and to alter it and redistribute it
//   freely, subject to the following restrictions:
//
//   1. The origin of this software must not be misrepresented; you must not
//      claim that you wrote the original software. If you use this software
//      in a product, an acknowledgment in the product documentation would be
//      appreciated but is not required.
//   2. Altered source versions must be plainly marked as such, and must not be
//      misrepresented as being the original software.
//   3. This notice may not be removed or altered from any source distribution.

use super::parser::sdl_element;
use super::select::sdl_guid;
use super::{DigitalHat, Mapping};
use crate::ev::{Axis, AxisOrBtn};

use fnv::FnvHashMap;
use gilrs_core::{EvCode, EvdevAbsInfo};

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

// linux/input-event-codes.h
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;
const KEY_RECORD: u16 = 167;
const BTN_JOYSTICK: u16 = 0x120;
const BTN_GAMEPAD: u16 = 0x130;
const BTN_A: u16 = 0x130;
const BTN_B: u16 = 0x131;
const BTN_X: u16 = 0x133;
const BTN_Y: u16 = 0x134;
const BTN_NORTH: u16 = 0x133;
const BTN_WEST: u16 = 0x134;
const BTN_TL: u16 = 0x136;
const BTN_TR: u16 = 0x137;
const BTN_TL2: u16 = 0x138;
const BTN_TR2: u16 = 0x139;
const BTN_SELECT: u16 = 0x13a;
const BTN_START: u16 = 0x13b;
const BTN_MODE: u16 = 0x13c;
const BTN_THUMBL: u16 = 0x13d;
const BTN_THUMBR: u16 = 0x13e;
const BTN_DPAD_UP: u16 = 0x220;
const BTN_DPAD_DOWN: u16 = 0x221;
const BTN_DPAD_LEFT: u16 = 0x222;
const BTN_DPAD_RIGHT: u16 = 0x223;
const BTN_GRIPL: u16 = 0x224;
const BTN_GRIPR: u16 = 0x225;
const BTN_GRIPL2: u16 = 0x226;
const BTN_GRIPR2: u16 = 0x227;
const BTN_TRIGGER_HAPPY1: u16 = 0x2c0;
const KEY_MAX: u16 = 0x2ff;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_Z: u16 = 0x02;
const ABS_RX: u16 = 0x03;
const ABS_RY: u16 = 0x04;
const ABS_RZ: u16 = 0x05;
const ABS_GAS: u16 = 0x09;
const ABS_BRAKE: u16 = 0x0a;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;
const ABS_HAT1X: u16 = 0x12;
const ABS_HAT1Y: u16 = 0x13;
const ABS_HAT2X: u16 = 0x14;
const ABS_HAT2Y: u16 = 0x15;
const ABS_HAT3Y: u16 = 0x17;
const ABS_MAX: u16 = 0x3f;
// src/joystick/usb_ids.h:48, 64
const USB_VENDOR_MICROSOFT: u16 = 0x045e;
const USB_VENDOR_SONY: u16 = 0x054c;
// SDL_hat.h: a hat's direction bits
const HAT_UP: u8 = 0x01;
const HAT_RIGHT: u8 = 0x02;
const HAT_DOWN: u8 = 0x04;
const HAT_LEFT: u8 = 0x08;

/// The evdev facts SDL's rule reads from a device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Capabilities {
    /// The `EV_KEY` codes the device reports (`EVIOCGBIT(EV_KEY)`).
    pub(crate) keys: BTreeSet<u16>,
    /// The `EV_ABS` codes the device reports (`EVIOCGBIT(EV_ABS)`) with their `EVIOCGABS` (`None`: the ioctl failed).
    pub(crate) abs: BTreeMap<u16, Option<EvdevAbsInfo>>,
    /// `SDL_GetJoystickVendor`: the USB vendor as SDL's GUID of the device carries it (0 when it carries none).
    pub(crate) vendor: u16,
    /// The device's driver as SDL's udev lookup names it.
    pub(crate) driver: Option<String>,
}

impl Capabilities {
    /// The facts of a device gilrs has opened.
    pub(crate) fn of(gamepad: &gilrs_core::Gamepad) -> Self {
        let keys = gamepad
            .buttons()
            .iter()
            .filter_map(|&c| evdev_code(c, EV_KEY))
            .collect();
        let abs = gamepad
            .axes()
            .iter()
            .filter_map(|&c| evdev_code(c, EV_ABS))
            .map(|a| (a, gamepad.evdev_abs_info(a)))
            .collect();
        Capabilities {
            keys,
            abs,
            vendor: sdl_guid::vendor(&gamepad.uuid()),
            driver: gamepad.udev_driver().map(str::to_owned),
        }
    }
}

/// The evdev code of `code` if it is of event type `kind`.
fn evdev_code(code: EvCode, kind: u16) -> Option<u16> {
    let packed = code.into_u32();
    (packed >> 16 == u32::from(kind)).then_some((packed & 0xffff) as u16)
}

/// `GuessIfAxesAreDigitalHat` with `SDL_HINT_JOYSTICK_LINUX_DIGITAL_HATS` at its default (off): an axis pair is a
/// digital hat when both present axes span exactly −1..1, or both lack fuzz, flat and resolution.
fn is_digital_hat(x: Option<EvdevAbsInfo>, y: Option<EvdevAbsInfo>) -> bool {
    if x.is_none() && y.is_none() {
        return false;
    }
    let unit = |i: Option<EvdevAbsInfo>| i.is_none_or(|i| i.minimum == -1 && i.maximum == 1);
    if unit(x) && unit(y) {
        return true;
    }
    let bare =
        |i: Option<EvdevAbsInfo>| i.is_none_or(|i| i.fuzz == 0 && i.flat == 0 && i.resolution == 0);
    bare(x) && bare(y)
}

/// What `ConfigJoystick` makes of a device: its buttons, axes and digital hats in SDL's index order.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Scan {
    /// `has_key`, in SDL's button order (`key_map`): `BTN_JOYSTICK..KEY_MAX`, then `0..BTN_JOYSTICK`.
    buttons: Vec<u16>,
    /// `has_abs`, in SDL's axis order (`abs_map`): every reported axis below `ABS_MAX` whose `EVIOCGABS` succeeds,
    /// except the axes of digital hats.
    axes: Vec<u16>,
    /// `has_hat` per evdev hat 0..3, with `hat_correct`'s starting range per axis (x, y): the axis' `EVIOCGABS`
    /// minimum and maximum, or −1 and 1 when SDL has none for it.
    hats: [Option<[(i32, i32); 2]>; 4],
}

impl Scan {
    fn new(caps: &Capabilities) -> Self {
        let mut hats = [None; 4];
        for (n, hat) in hats.iter_mut().enumerate() {
            let axis = |code: u16| caps.abs.get(&code).copied().flatten();
            let (x, y) = (
                axis(ABS_HAT0X + 2 * n as u16),
                axis(ABS_HAT0X + 2 * n as u16 + 1),
            );
            if is_digital_hat(x, y) {
                let range = |i: Option<EvdevAbsInfo>| i.map_or((-1, 1), |i| (i.minimum, i.maximum));
                *hat = Some([range(x), range(y)]);
            }
        }
        let below_joystick = caps.keys.iter().copied().filter(|&k| k < BTN_JOYSTICK);
        let buttons = (caps.keys.iter().copied())
            .filter(|&k| (BTN_JOYSTICK..KEY_MAX).contains(&k))
            .chain(below_joystick)
            .collect();
        let axes = (caps.abs.iter())
            .filter(|&(&a, info)| a < ABS_MAX && info.is_some() && !Self::is_hat_axis(&hats, a))
            .map(|(&a, _)| a)
            .collect();
        Scan {
            buttons,
            axes,
            hats,
        }
    }

    fn is_hat_axis(hats: &[Option<[(i32, i32); 2]>; 4], code: u16) -> bool {
        (ABS_HAT0X..=ABS_HAT3Y).contains(&code)
            && hats[usize::from((code - ABS_HAT0X) / 2)].is_some()
    }

    fn has_key(&self, code: u16) -> bool {
        self.buttons.contains(&code)
    }

    fn has_abs(&self, code: u16) -> bool {
        self.axes.contains(&code)
    }

    fn has_hat(&self, hat: u8) -> bool {
        self.hats[usize::from(hat)].is_some()
    }
}

/// One input of SDL's mapping: an evdev key, an evdev axis (whole), or a direction of digital hat `hat` (the evdev
/// hat number, 0..3; SDL's row names it by its index among the device's digital hats).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Input {
    Button(u16),
    Axis(u16),
    Hat { hat: u8, mask: u8 },
}

/// `SDL_GamepadMapping` as the Linux rule fills it. SDL's `misc2`..`misc6` and `touchpad` are left out: the rule
/// never sets them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SdlMapping {
    pub(crate) a: Option<Input>,
    pub(crate) b: Option<Input>,
    pub(crate) x: Option<Input>,
    pub(crate) y: Option<Input>,
    pub(crate) back: Option<Input>,
    pub(crate) guide: Option<Input>,
    pub(crate) start: Option<Input>,
    pub(crate) leftstick: Option<Input>,
    pub(crate) rightstick: Option<Input>,
    pub(crate) leftshoulder: Option<Input>,
    pub(crate) rightshoulder: Option<Input>,
    pub(crate) dpup: Option<Input>,
    pub(crate) dpdown: Option<Input>,
    pub(crate) dpleft: Option<Input>,
    pub(crate) dpright: Option<Input>,
    pub(crate) misc1: Option<Input>,
    pub(crate) right_paddle1: Option<Input>,
    pub(crate) left_paddle1: Option<Input>,
    pub(crate) right_paddle2: Option<Input>,
    pub(crate) left_paddle2: Option<Input>,
    pub(crate) leftx: Option<Input>,
    pub(crate) lefty: Option<Input>,
    pub(crate) rightx: Option<Input>,
    pub(crate) righty: Option<Input>,
    pub(crate) lefttrigger: Option<Input>,
    pub(crate) righttrigger: Option<Input>,
}

impl SdlMapping {
    /// The elements in the order `SDL_PrivateGenerateAutomaticGamepadMapping` writes them, under the names it writes
    /// (`paddle1`..`paddle4` for the right/left paddles). SDL binds an input to the first of these that names it
    /// (`HandleJoystickAxis` / `HandleJoystickButton` stop at the first binding, SDL_gamepad.c:247-321).
    pub(crate) fn elements(&self) -> [(&'static str, Option<Input>); 26] {
        [
            ("a", self.a),
            ("b", self.b),
            ("x", self.x),
            ("y", self.y),
            ("back", self.back),
            ("guide", self.guide),
            ("start", self.start),
            ("leftstick", self.leftstick),
            ("rightstick", self.rightstick),
            ("leftshoulder", self.leftshoulder),
            ("rightshoulder", self.rightshoulder),
            ("dpup", self.dpup),
            ("dpdown", self.dpdown),
            ("dpleft", self.dpleft),
            ("dpright", self.dpright),
            ("misc1", self.misc1),
            ("paddle1", self.right_paddle1),
            ("paddle2", self.left_paddle1),
            ("paddle3", self.right_paddle2),
            ("paddle4", self.left_paddle2),
            ("leftx", self.leftx),
            ("lefty", self.lefty),
            ("rightx", self.rightx),
            ("righty", self.righty),
            ("lefttrigger", self.lefttrigger),
            ("righttrigger", self.righttrigger),
        ]
    }
}

/// SDL's automatic mapping of one device: the rule's result and the scan it was made from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Generated {
    pub(crate) sdl: SdlMapping,
    scan: Scan,
}

/// `LINUX_JoystickGetGamepadMapping`: SDL's mapping of a device, or `None` when SDL makes none (the device does not
/// report `BTN_GAMEPAD`: "Not a gamepad according to the specs").
pub(crate) fn generate(caps: &Capabilities) -> Option<Generated> {
    let scan = Scan::new(caps);
    if !scan.has_key(BTN_GAMEPAD) {
        return None;
    }
    let key = |code: u16| scan.has_key(code).then_some(Input::Button(code));
    let abs = |code: u16| scan.has_abs(code).then_some(Input::Axis(code));
    let hat = |hat: u8, mask: u8| Some(Input::Hat { hat, mask });
    let mut m = SdlMapping {
        a: key(BTN_A),
        b: key(BTN_B),
        back: key(BTN_SELECT),
        start: key(BTN_START),
        leftstick: key(BTN_THUMBL),
        rightstick: key(BTN_THUMBR),
        guide: key(BTN_MODE),
        ..SdlMapping::default()
    };
    // Xbox controllers use BTN_X and BTN_Y, and PS4 controllers use BTN_WEST and BTN_NORTH.
    if caps.vendor == USB_VENDOR_SONY {
        (m.x, m.y) = (key(BTN_WEST), key(BTN_NORTH));
    } else {
        (m.x, m.y) = (key(BTN_X), key(BTN_Y));
    }

    // Prefer digital shoulder buttons, but settle for digital or analog hat.
    let (mut left, mut right) = (false, false);
    if let Some(input) = key(BTN_TL) {
        (m.leftshoulder, left) = (Some(input), true);
    }
    if let Some(input) = key(BTN_TR) {
        (m.rightshoulder, right) = (Some(input), true);
    }
    if !(left && right) && scan.has_hat(1) {
        (m.leftshoulder, m.rightshoulder) = (hat(1, HAT_DOWN), hat(1, HAT_RIGHT));
        (left, right) = (true, true);
    }
    if !left {
        m.leftshoulder = abs(ABS_HAT1Y);
    }
    if !right {
        m.rightshoulder = abs(ABS_HAT1X);
    }

    // Prefer analog triggers (Linux gamepad specification HAT2Y/HAT2X, Android BRAKE/GAS, older Xbox and PlayStation
    // pads Z/RZ, in turn), but settle for a digital hat or buttons.
    m.lefttrigger = abs(ABS_HAT2Y)
        .or_else(|| abs(ABS_BRAKE))
        .or_else(|| abs(ABS_Z));
    m.righttrigger = abs(ABS_HAT2X)
        .or_else(|| abs(ABS_GAS))
        .or_else(|| abs(ABS_RZ));
    if !(m.lefttrigger.is_some() && m.righttrigger.is_some()) && scan.has_hat(2) {
        (m.lefttrigger, m.righttrigger) = (hat(2, HAT_DOWN), hat(2, HAT_RIGHT));
    }
    if m.lefttrigger.is_none() {
        m.lefttrigger = key(BTN_TL2);
    }
    if m.righttrigger.is_none() {
        m.righttrigger = key(BTN_TR2);
    }

    // Prefer digital D-Pad buttons, but settle for digital or analog hat.
    m.dpup = key(BTN_DPAD_UP);
    m.dpdown = key(BTN_DPAD_DOWN);
    m.dpleft = key(BTN_DPAD_LEFT);
    m.dpright = key(BTN_DPAD_RIGHT);
    let dpad = [m.dpup, m.dpdown, m.dpleft, m.dpright];
    if dpad.iter().any(Option::is_none) {
        if scan.has_hat(0) {
            m.dpleft = hat(0, HAT_LEFT);
            m.dpright = hat(0, HAT_RIGHT);
            m.dpup = hat(0, HAT_UP);
            m.dpdown = hat(0, HAT_DOWN);
        } else if scan.has_abs(ABS_HAT0X) && scan.has_abs(ABS_HAT0Y) {
            (m.dpleft, m.dpright) = (abs(ABS_HAT0X), abs(ABS_HAT0X));
            (m.dpup, m.dpdown) = (abs(ABS_HAT0Y), abs(ABS_HAT0Y));
        } else if caps.driver.as_deref() == Some("xpad") {
            // xpad will sometimes map the D-Pad as BTN_TRIGGER_HAPPY1 - BTN_TRIGGER_HAPPY4.
            let happy: [Option<Input>; 4] =
                std::array::from_fn(|i| key(BTN_TRIGGER_HAPPY1 + i as u16));
            if happy.iter().all(Option::is_some) {
                [m.dpleft, m.dpright, m.dpup, m.dpdown] = happy;
            }
        }
    }

    if scan.has_abs(ABS_X) && scan.has_abs(ABS_Y) {
        (m.leftx, m.lefty) = (abs(ABS_X), abs(ABS_Y));
    }
    // The Linux gamepad specification's RX/RY second stick, else the Android convention's Z/RZ.
    if scan.has_abs(ABS_RX) && scan.has_abs(ABS_RY) {
        (m.rightx, m.righty) = (abs(ABS_RX), abs(ABS_RY));
    } else if scan.has_abs(ABS_Z) && scan.has_abs(ABS_RZ) {
        (m.rightx, m.righty) = (abs(ABS_Z), abs(ABS_RZ));
    }

    m.right_paddle1 = key(BTN_GRIPR);
    m.left_paddle1 = key(BTN_GRIPL);
    m.right_paddle2 = key(BTN_GRIPR2);
    m.left_paddle2 = key(BTN_GRIPL2);
    let paddles = [
        m.right_paddle1,
        m.left_paddle1,
        m.right_paddle2,
        m.left_paddle2,
    ];
    if paddles.iter().any(Option::is_none) && caps.vendor == USB_VENDOR_MICROSOFT {
        // The Xbox Elite controllers have the paddles as BTN_TRIGGER_HAPPY5 - BTN_TRIGGER_HAPPY8 in older drivers.
        let happy: [Option<Input>; 4] =
            std::array::from_fn(|i| key(BTN_TRIGGER_HAPPY1 + 4 + i as u16));
        if happy.iter().all(Option::is_some) {
            [
                m.right_paddle1,
                m.right_paddle2,
                m.left_paddle1,
                m.left_paddle2,
            ] = happy;
        }
    }

    // Xbox Series controllers have the Share button as KEY_RECORD.
    m.misc1 = key(KEY_RECORD);

    Some(Generated { sdl: m, scan })
}

impl Generated {
    /// SDL's index of each input: the key's place among `has_key` in SDL's order, the axis' among `has_abs`, the
    /// hat's among the digital hats (`hats_indices`). Every input of the rule is one the scan found.
    fn index(&self, input: Input) -> Option<usize> {
        match input {
            Input::Button(code) => self.scan.buttons.iter().position(|&k| k == code),
            Input::Axis(code) => self.scan.axes.iter().position(|&a| a == code),
            Input::Hat { hat, .. } => {
                let hat = usize::from(hat);
                (self.scan.hats.get(hat)?.is_some())
                    .then(|| self.scan.hats[..hat].iter().flatten().count())
            }
        }
    }

    /// The elements of the row SDL adds for the device (`SDL_PrivateGenerateAutomaticGamepadMapping`), as SDL writes
    /// them: `a:b0,b:b1,...,` (buttons `b`, axes `a`, hats `h<hat>.<direction>`, SDL's indices).
    pub(crate) fn sdl_elements(&self) -> String {
        let mut row = String::new();
        for (element, input) in self.sdl.elements() {
            let (Some(input), Some(index)) = (input, input.and_then(|i| self.index(i))) else {
                continue;
            };
            let _ = match input {
                Input::Button(_) => write!(row, "{element}:b{index},"),
                Input::Axis(_) => write!(row, "{element}:a{index},"),
                Input::Hat { mask, .. } => write!(row, "{element}:h{index}.{mask},"),
            };
        }
        row
    }

    /// The gilrs mapping of SDL's: each input bound to the first element that names it (SDL's first match), as the
    /// gilrs element gilrs' row parser gives that element's name (an axis bound to `lefttrigger` is the button
    /// `LeftTrigger2` with the axis' value, as for a row's `lefttrigger:a2`). A hat direction is read as SDL's
    /// `HandleHat` reads it ([`DigitalHat`]); hat 0's directions on the d-pad are the axes `DPadX` / `DPadY`, which
    /// gilrs' d-pad filter turns into the d-pad buttons, as it does for a row's `h0.*`.
    pub(crate) fn mapping(&self, name: &str) -> Mapping {
        let mut mapping = Mapping::new();
        mapping.name = sdl_name(name);
        mapping.automatic = true;
        let mut digital_hats: FnvHashMap<EvCode, DigitalHat> = FnvHashMap::default();
        for (element, input) in self.sdl.elements() {
            let (Some(input), Some(target)) = (input, sdl_element(element)) else {
                continue;
            };
            let dpad = match element {
                "dpleft" => Some((Axis::DPadX, HAT_LEFT, BTN_DPAD_LEFT)),
                "dpright" => Some((Axis::DPadX, HAT_RIGHT, BTN_DPAD_RIGHT)),
                "dpup" => Some((Axis::DPadY, HAT_UP, BTN_DPAD_UP)),
                "dpdown" => Some((Axis::DPadY, HAT_DOWN, BTN_DPAD_DOWN)),
                _ => None,
            };
            // A d-pad direction on a hat or hat axes: gilrs' d-pad axis, its direction counted in `hats_mapped`, and
            // the native d-pad button the d-pad filter reports it as bound to the button, as gilrs' parser does for a
            // row's `h0.*`.
            let dpad_axis = |mapping: &mut Mapping, axis: Axis, direction: u8, button: u16| {
                mapping.hats_mapped |= direction;
                mapping
                    .mappings
                    .entry(EvCode::from_evdev(EV_KEY, button))
                    .or_insert(target);
                AxisOrBtn::Axis(axis)
            };
            match input {
                Input::Button(code) => {
                    mapping
                        .mappings
                        .entry(EvCode::from_evdev(EV_KEY, code))
                        .or_insert(target);
                }
                Input::Axis(code) => {
                    // An analog hat 0 as the d-pad: SDL binds each direction to the whole axis (`dpup:a7,dpdown:a7`),
                    // and a whole axis reads as a pressed button from its middle up (`HandleJoystickAxis`'s threshold
                    // −1 of SDL's −32768..32767), so SDL itself reads up and left as held at rest; gilrs' parser keeps
                    // only the last element per input (down and right). The port reads the axes as gilrs' d-pad axes.
                    // PORT(choice): an analog ABS_HAT0X/ABS_HAT0Y d-pad (no BTN_DPAD_* buttons, no digital hat 0)
                    // becomes gilrs' DPadX/DPadY, not SDL 3.4.20's whole-axis buttons (SDL_sysjoystick.c:2658-2671),
                    // which read up and left as held at rest; the console read XInput pads only, which need no mapping.
                    let to = match dpad {
                        Some((axis, direction, button)) => {
                            dpad_axis(&mut mapping, axis, direction, button)
                        }
                        None => target,
                    };
                    mapping
                        .mappings
                        .entry(EvCode::from_evdev(EV_ABS, code))
                        .or_insert(to);
                }
                Input::Hat { hat, mask } => {
                    let (axis, side) = match mask {
                        HAT_LEFT => (0, -1),
                        HAT_RIGHT => (0, 1),
                        HAT_UP => (1, -1),
                        HAT_DOWN => (1, 1),
                        _ => continue,
                    };
                    let Some(range) = self.scan.hats[usize::from(hat)] else {
                        continue;
                    };
                    let code = EvCode::from_evdev(EV_ABS, ABS_HAT0X + 2 * u16::from(hat) + axis);
                    let (to, press) = match dpad {
                        Some((axis, direction, button)) => {
                            (dpad_axis(&mut mapping, axis, direction, button), None)
                        }
                        None => (target, Some(side)),
                    };
                    if !mapping.mappings.contains_key(&code) {
                        mapping.mappings.insert(code, to);
                        let (minimum, maximum) = range[usize::from(axis)];
                        digital_hats.insert(
                            code,
                            DigitalHat {
                                minimum,
                                maximum,
                                press,
                            },
                        );
                    }
                }
            }
        }
        mapping.digital_hats = digital_hats;
        mapping
    }
}

/// The name SDL gives the generated row: the device's name with every comma made a space, cut to 127 bytes
/// (`name_string[128]`).
fn sdl_name(name: &str) -> String {
    let mut name = name.replace(',', " ");
    // PORT(choice): SDL's strlcpy cuts the name at 127 bytes even inside a UTF-8 sequence; a Rust string cannot hold
    // half a character, so the cut moves back to the character's start (only the mapping's name, `Gamepad::map_name`,
    // carries it); the console read XInput pads only, which need no mapping.
    let mut end = name.len().min(127);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name.truncate(end);
    name
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ev::Button;

    fn info(minimum: i32, maximum: i32, fuzz: i32, flat: i32) -> Option<EvdevAbsInfo> {
        Some(EvdevAbsInfo {
            minimum,
            maximum,
            fuzz,
            flat,
            resolution: 0,
        })
    }

    const STICK: Option<EvdevAbsInfo> = Some(EvdevAbsInfo {
        minimum: -32768,
        maximum: 32767,
        fuzz: 16,
        flat: 128,
        resolution: 0,
    });
    const HAT: Option<EvdevAbsInfo> = Some(EvdevAbsInfo {
        minimum: -1,
        maximum: 1,
        fuzz: 0,
        flat: 0,
        resolution: 0,
    });
    const XPAD_KEYS: [u16; 11] = [
        BTN_A, BTN_B, BTN_X, BTN_Y, BTN_TL, BTN_TR, BTN_SELECT, BTN_START, BTN_MODE, BTN_THUMBL,
        BTN_THUMBR,
    ];

    fn caps(keys: &[u16], abs: &[(u16, Option<EvdevAbsInfo>)]) -> Capabilities {
        Capabilities {
            keys: keys.iter().copied().collect(),
            abs: abs.iter().copied().collect(),
            vendor: 0x1234,
            driver: None,
        }
    }

    /// The xpad driver's Xbox 360 layout: sticks, Z/RZ 0..255 triggers, a −1..1 hat 0.
    fn xpad() -> Capabilities {
        let trigger = info(0, 255, 0, 0);
        let abs = [
            (ABS_X, STICK),
            (ABS_Y, STICK),
            (ABS_Z, trigger),
            (ABS_RX, STICK),
            (ABS_RY, STICK),
        ];
        caps(
            &XPAD_KEYS,
            &[
                &abs[..],
                &[(ABS_RZ, trigger), (ABS_HAT0X, HAT), (ABS_HAT0Y, HAT)],
            ]
            .concat(),
        )
    }

    fn map(mapping: &Mapping, kind: u16, code: u16) -> Option<AxisOrBtn> {
        mapping.map(&EvCode::from_evdev(kind, code))
    }

    #[test]
    fn xpad_layout_is_sdls_and_its_triggers_are_the_trigger_buttons() {
        let generated = generate(&xpad()).expect("BTN_GAMEPAD makes a mapping");
        assert_eq!(
            generated.sdl_elements(),
            "a:b0,b:b1,x:b2,y:b3,back:b6,guide:b8,start:b7,leftstick:b9,rightstick:b10,leftshoulder:b4,\
             rightshoulder:b5,dpup:h0.1,dpdown:h0.4,dpleft:h0.8,dpright:h0.2,leftx:a0,lefty:a1,rightx:a3,righty:a4,\
             lefttrigger:a2,righttrigger:a5,"
        );
        let mapping = generated.mapping("X360, pad");
        assert_eq!(mapping.name(), "X360  pad");
        assert!(mapping.is_automatic() && !mapping.is_default());
        assert_eq!(
            map(&mapping, EV_ABS, ABS_Z),
            Some(AxisOrBtn::Btn(Button::LeftTrigger2))
        );
        assert_eq!(
            map(&mapping, EV_ABS, ABS_RZ),
            Some(AxisOrBtn::Btn(Button::RightTrigger2))
        );
        assert_eq!(
            map(&mapping, EV_ABS, ABS_RX),
            Some(AxisOrBtn::Axis(Axis::RightStickX))
        );
        assert_eq!(
            map(&mapping, EV_ABS, ABS_HAT0Y),
            Some(AxisOrBtn::Axis(Axis::DPadY))
        );
        assert_eq!(
            map(&mapping, EV_KEY, BTN_TL),
            Some(AxisOrBtn::Btn(Button::LeftTrigger))
        );
        assert_eq!(
            map(&mapping, EV_KEY, BTN_MODE),
            Some(AxisOrBtn::Btn(Button::Mode))
        );
        assert_eq!(mapping.hats_mapped(), 0x0f);
        assert_eq!(
            map(&mapping, EV_KEY, BTN_DPAD_UP),
            Some(AxisOrBtn::Btn(Button::DPadUp))
        );
        assert_eq!(
            map(&mapping, EV_KEY, BTN_DPAD_LEFT),
            Some(AxisOrBtn::Btn(Button::DPadLeft))
        );
    }

    #[test]
    fn no_btn_gamepad_no_mapping() {
        // A joystick (BTN_TRIGGER, BTN_THUMB...) and a pad with only face keys below BTN_GAMEPAD.
        assert_eq!(
            generate(&caps(
                &[0x120, 0x121, 0x122],
                &[(ABS_X, STICK), (ABS_Y, STICK)]
            )),
            None
        );
        assert_eq!(
            generate(&caps(
                &[BTN_B, BTN_X, BTN_TL2],
                &[(ABS_Z, STICK), (ABS_RZ, STICK)]
            )),
            None
        );
    }

    #[test]
    fn without_rx_ry_z_rz_are_the_right_stick_first() {
        let abs = [
            (ABS_X, STICK),
            (ABS_Y, STICK),
            (ABS_Z, STICK),
            (ABS_RZ, STICK),
        ];
        let generated = generate(&caps(&[BTN_A, BTN_B, BTN_TL2, BTN_TR2], &abs)).unwrap();
        // SDL binds Z/RZ twice; the stick comes first in the row, so it is what Z and RZ do (and BTN_TL2/TR2 are not
        // used: the triggers counted as mapped).
        assert!(generated
            .sdl_elements()
            .ends_with("rightx:a2,righty:a3,lefttrigger:a2,righttrigger:a3,"));
        let mapping = generated.mapping("android");
        assert_eq!(
            map(&mapping, EV_ABS, ABS_Z),
            Some(AxisOrBtn::Axis(Axis::RightStickX))
        );
        assert_eq!(
            map(&mapping, EV_ABS, ABS_RZ),
            Some(AxisOrBtn::Axis(Axis::RightStickY))
        );
        assert_eq!(map(&mapping, EV_KEY, BTN_TL2), None);

        // The Android convention's BRAKE/GAS triggers beside a Z/RZ stick.
        let trigger = info(0, 1023, 0, 0);
        let abs = [&abs[..], &[(ABS_GAS, trigger), (ABS_BRAKE, trigger)]].concat();
        let mapping = generate(&caps(&[BTN_A], &abs)).unwrap().mapping("android");
        assert_eq!(
            map(&mapping, EV_ABS, ABS_BRAKE),
            Some(AxisOrBtn::Btn(Button::LeftTrigger2))
        );
        assert_eq!(
            map(&mapping, EV_ABS, ABS_GAS),
            Some(AxisOrBtn::Btn(Button::RightTrigger2))
        );
        assert_eq!(
            map(&mapping, EV_ABS, ABS_Z),
            Some(AxisOrBtn::Axis(Axis::RightStickX))
        );
    }

    #[test]
    fn only_digital_tl2_tr2_are_the_triggers() {
        let mapping = generate(&caps(
            &[BTN_A, BTN_TL2, BTN_TR2],
            &[(ABS_X, STICK), (ABS_Y, STICK)],
        ))
        .unwrap();
        assert_eq!(
            mapping.sdl_elements(),
            "a:b0,leftx:a0,lefty:a1,lefttrigger:b1,righttrigger:b2,"
        );
        let mapping = mapping.mapping("digital");
        assert_eq!(
            map(&mapping, EV_KEY, BTN_TL2),
            Some(AxisOrBtn::Btn(Button::LeftTrigger2))
        );
        assert_eq!(
            map(&mapping, EV_KEY, BTN_TR2),
            Some(AxisOrBtn::Btn(Button::RightTrigger2))
        );
    }

    #[test]
    fn hats_as_triggers_digital_and_analog() {
        let sticks = [
            (ABS_X, STICK),
            (ABS_Y, STICK),
            (ABS_RX, STICK),
            (ABS_RY, STICK),
        ];
        let digital = [&sticks[..], &[(ABS_HAT2X, HAT), (ABS_HAT2Y, HAT)]].concat();
        let generated = generate(&caps(&[BTN_A, BTN_TL, BTN_TR], &digital)).unwrap();
        assert!(generated
            .sdl_elements()
            .ends_with("righty:a3,lefttrigger:h0.4,righttrigger:h0.2,"));
        let mut mapping = generated.mapping("hat triggers");
        let hat2y = EvCode::from_evdev(EV_ABS, ABS_HAT2Y);
        assert_eq!(
            mapping.map(&hat2y),
            Some(AxisOrBtn::Btn(Button::LeftTrigger2))
        );
        // Down (positive) presses the left trigger; up and the centre do not.
        assert_eq!(mapping.digital_hat_value(hat2y, 1), Some(1.0));
        assert_eq!(mapping.digital_hat_value(hat2y, -1), Some(0.0));
        assert_eq!(mapping.digital_hat_value(hat2y, 0), Some(0.0));
        assert_eq!(
            mapping.digital_hat_value(EvCode::from_evdev(EV_ABS, ABS_HAT2X), 1),
            Some(1.0)
        );
        assert_eq!(
            mapping.digital_hat_value(EvCode::from_evdev(EV_ABS, ABS_RX), 1),
            None
        );

        // With fuzz, flat or resolution and a range other than −1..1, hat 2 is analog: two trigger axes.
        let analog = info(0, 255, 0, 15);
        let abs = [&sticks[..], &[(ABS_HAT2X, analog), (ABS_HAT2Y, analog)]].concat();
        let generated = generate(&caps(&[BTN_A], &abs)).unwrap();
        assert!(generated
            .sdl_elements()
            .ends_with("lefttrigger:a5,righttrigger:a4,"));
        let mapping = generated.mapping("analog hat triggers");
        assert_eq!(
            map(&mapping, EV_ABS, ABS_HAT2Y),
            Some(AxisOrBtn::Btn(Button::LeftTrigger2))
        );
        assert!(mapping.digital_hats.is_empty());
    }

    #[test]
    fn hat_1_shoulders_override_half_mapped_buttons() {
        let abs = [
            (ABS_X, STICK),
            (ABS_Y, STICK),
            (ABS_HAT0X, HAT),
            (ABS_HAT0Y, HAT),
            (ABS_HAT1X, HAT),
        ];
        let generated = generate(&caps(
            &[BTN_A, BTN_TL],
            &[&abs[..], &[(ABS_HAT1Y, HAT)]].concat(),
        ))
        .unwrap();
        assert_eq!(
            generated.sdl_elements(),
            "a:b0,leftshoulder:h1.4,rightshoulder:h1.2,dpup:h0.1,dpdown:h0.4,dpleft:h0.8,dpright:h0.2,leftx:a0,lefty:a1,"
        );
        let mapping = generated.mapping("hat shoulders");
        assert_eq!(
            map(&mapping, EV_ABS, ABS_HAT1Y),
            Some(AxisOrBtn::Btn(Button::LeftTrigger))
        );
        assert_eq!(map(&mapping, EV_KEY, BTN_TL), None);
    }

    #[test]
    fn sony_face_buttons_xpad_dpad_and_xbox_paddles() {
        let mut sony = caps(&[BTN_A, BTN_B, BTN_NORTH, BTN_WEST], &[]);
        sony.vendor = USB_VENDOR_SONY;
        assert_eq!(
            generate(&sony).unwrap().sdl_elements(),
            "a:b0,b:b1,x:b3,y:b2,"
        );

        let happy: Vec<u16> = (0..8).map(|i| BTN_TRIGGER_HAPPY1 + i).collect();
        let mut pad = caps(&[&XPAD_KEYS[..], &happy[..], &[KEY_RECORD]].concat(), &[]);
        let without_driver = generate(&pad).unwrap().sdl_elements();
        assert!(!without_driver.contains("dp"));
        pad.driver = Some("xpad".to_owned());
        pad.vendor = USB_VENDOR_MICROSOFT;
        let row = generate(&pad).unwrap().sdl_elements();
        assert!(
            row.contains("dpup:b13,dpdown:b14,dpleft:b11,dpright:b12,misc1:b19,"),
            "{row}"
        );
        assert!(
            row.contains("paddle1:b15,paddle2:b17,paddle3:b16,paddle4:b18,"),
            "{row}"
        );
        let mapping = generate(&pad).unwrap().mapping("xpad");
        assert_eq!(
            map(&mapping, EV_KEY, BTN_TRIGGER_HAPPY1),
            Some(AxisOrBtn::Btn(Button::DPadLeft))
        );
        assert_eq!(mapping.hats_mapped(), 0);
    }

    #[test]
    fn digital_hat_guess_and_handle_hat() {
        assert!(!is_digital_hat(None, None));
        assert!(is_digital_hat(HAT, None));
        assert!(is_digital_hat(info(-100, 100, 0, 0), info(-100, 100, 0, 0)));
        assert!(!is_digital_hat(HAT, info(0, 255, 4, 0)));
        assert!(!is_digital_hat(info(-127, 127, 0, 1), None));

        let mut hat = DigitalHat {
            minimum: -100,
            maximum: 100,
            press: None,
        };
        // Beyond a third of the extreme is a direction; past the extreme the extreme grows.
        assert_eq!(
            [
                hat.position(-40),
                hat.position(-33),
                hat.position(0),
                hat.position(34)
            ],
            [-1, 0, 0, 1]
        );
        assert_eq!(hat.position(150), 1);
        assert_eq!(
            (hat.maximum, hat.position(60), hat.position(50)),
            (150, 1, 0)
        );
        // C's division: −200 / 3 is −66.
        assert_eq!(
            (
                hat.position(-200),
                hat.minimum,
                hat.position(-67),
                hat.position(-66)
            ),
            (-1, -200, -1, 0)
        );
    }

    #[test]
    fn generated_name_drops_commas_and_stops_at_127_bytes() {
        assert_eq!(sdl_name("a,b"), "a b");
        let long = sdl_name(&"é".repeat(100));
        assert_eq!(long.len(), 126);
        assert!(long.chars().all(|c| c == 'é'));
    }
}
