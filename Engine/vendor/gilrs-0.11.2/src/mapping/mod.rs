// Copyright 2016-2018 Mateusz Sieczko and other GilRs Developers
//
// Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
// http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
// http://opensource.org/licenses/MIT>, at your option. This file may not be
// copied, modified, or distributed except according to those terms.
#![cfg_attr(target_os = "windows", allow(dead_code))]

mod parser;
// Project Avalanche patch T22 / T23: SDL's mapping lookup order and its automatic gamepad mapping (Linux).
#[cfg(target_os = "linux")]
pub(crate) mod automatic;
#[cfg(target_os = "linux")]
pub(crate) mod select;
#[cfg(target_os = "linux")]
use select::sdl_guid;

use crate::ev::{self, Axis, AxisOrBtn, Button};
use gilrs_core::native_ev_codes as nec;
use gilrs_core::EvCode;

use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fmt::{Display, Formatter, Result as FmtResult, Write as _};

use fnv::FnvHashMap;
use uuid::Uuid;
use vec_map::VecMap;

use self::parser::{Error as ParserError, ErrorKind as ParserErrorKind, Parser, Token};

/// Platform name used by SDL mappings
#[cfg(any(target_os = "linux", target_os = "dragonfly", target_os = "freebsd"))]
const SDL_PLATFORM_NAME: &str = "Linux";
#[cfg(target_os = "macos")]
const SDL_PLATFORM_NAME: &str = "Mac OS X";
#[cfg(target_os = "windows")]
const SDL_PLATFORM_NAME: &str = "Windows";
#[cfg(all(
    not(any(target_os = "linux", target_os = "dragonfly", target_os = "freebsd")),
    not(target_os = "macos"),
    not(target_os = "windows")
))]
const SDL_PLATFORM_NAME: &str = "Unknown";

#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
/// Store mappings from one `EvCode` (`u16`) to another.
///
/// This struct is internal, `MappingData` is exported in public interface as `Mapping`.
pub struct Mapping {
    mappings: FnvHashMap<EvCode, AxisOrBtn>,
    name: String,
    default: bool,
    hats_mapped: u8,
    // Project Avalanche patch T23: the axes SDL's automatic mapping reads as digital hats (SDL's `HandleHat`), and
    // whether this is that mapping.
    #[cfg(target_os = "linux")]
    digital_hats: FnvHashMap<EvCode, DigitalHat>,
    #[cfg(target_os = "linux")]
    automatic: bool,
}

/// Project Avalanche patch T23: an axis of a hat SDL reads as digital (`GuessIfAxesAreDigitalHat`,
/// SDL 3.4.20 `src/joystick/linux/SDL_sysjoystick.c:1192-1223`), with `HandleHat`'s state (1730-1777): the
/// `hat_axis_correct` minimum and maximum, which grow to a value beyond them. `press`: for a hat direction bound to a
/// button, the side (−1 or +1) that presses it; `None` for the d-pad's axes.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DigitalHat {
    pub(crate) minimum: i32,
    pub(crate) maximum: i32,
    pub(crate) press: Option<i8>,
}

#[cfg(target_os = "linux")]
impl DigitalHat {
    /// `HandleHat`'s position of the raw `value` on this axis: −1, 0 or +1. SDL's hat dead zones are on
    /// (`SDL_HINT_JOYSTICK_LINUX_HAT_DEADZONES`' default): between the centre and a third of the extreme is centred.
    // PORT(choice): SDL's hints SDL_JOYSTICK_LINUX_HAT_DEADZONES (default on) and SDL_JOYSTICK_LINUX_DIGITAL_HATS
    // (default off) are taken at their defaults and not read from the environment (gilrs reads no SDL hint but
    // SDL_GAMECONTROLLERCONFIG); the console read XInput pads only, which have no evdev hats.
    pub(crate) fn position(&mut self, value: i32) -> i8 {
        if value < 0 {
            if value <= self.minimum {
                self.minimum = value;
                -1
            } else if value < self.minimum / 3 {
                -1
            } else {
                0
            }
        } else if value > 0 {
            if value >= self.maximum {
                self.maximum = value;
                1
            } else if value > self.maximum / 3 {
                1
            } else {
                0
            }
        } else {
            0
        }
    }
}

impl Mapping {
    pub fn new() -> Self {
        Mapping {
            mappings: FnvHashMap::default(),
            name: String::new(),
            default: false,
            hats_mapped: 0,
            #[cfg(target_os = "linux")]
            digital_hats: FnvHashMap::default(),
            #[cfg(target_os = "linux")]
            automatic: false,
        }
    }

    pub fn default(gamepad: &gilrs_core::Gamepad) -> Self {
        use self::Axis as Ax;
        use self::AxisOrBtn::*;

        macro_rules! fnv_map {
            ( $( $key:expr => $elem:expr ),* ) => {
                {
                    let mut map = FnvHashMap::default();
                    $(
                        map.insert($key, $elem);
                    )*

                    map
                }
            };
        }

        let mut mappings = fnv_map![
            nec::BTN_SOUTH => Btn(Button::South),
            nec::BTN_EAST => Btn(Button::East),
            nec::BTN_C => Btn(Button::C),
            nec::BTN_NORTH => Btn(Button::North),
            nec::BTN_WEST => Btn(Button::West),
            nec::BTN_Z => Btn(Button::Z),
            nec::BTN_LT => Btn(Button::LeftTrigger),
            nec::BTN_RT => Btn(Button::RightTrigger),
            nec::BTN_LT2 => Btn(Button::LeftTrigger2),
            nec::BTN_RT2 => Btn(Button::RightTrigger2),
            nec::BTN_SELECT => Btn(Button::Select),
            nec::BTN_START => Btn(Button::Start),
            nec::BTN_MODE => Btn(Button::Mode),
            nec::BTN_LTHUMB => Btn(Button::LeftThumb),
            nec::BTN_RTHUMB => Btn(Button::RightThumb),
            nec::BTN_DPAD_UP => Btn(Button::DPadUp),
            nec::BTN_DPAD_DOWN => Btn(Button::DPadDown),
            nec::BTN_DPAD_LEFT => Btn(Button::DPadLeft),
            nec::BTN_DPAD_RIGHT => Btn(Button::DPadRight),

            nec::AXIS_LT => Btn(Button::LeftTrigger),
            nec::AXIS_RT => Btn(Button::RightTrigger),
            nec::AXIS_LT2 => Btn(Button::LeftTrigger2),
            nec::AXIS_RT2 => Btn(Button::RightTrigger2),

            nec::AXIS_LSTICKX => Axis(Ax::LeftStickX),
            nec::AXIS_LSTICKY => Axis(Ax::LeftStickY),
            nec::AXIS_LEFTZ => Axis(Ax::LeftZ),
            nec::AXIS_RSTICKX => Axis(Ax::RightStickX),
            nec::AXIS_RSTICKY => Axis(Ax::RightStickY),
            nec::AXIS_RIGHTZ => Axis(Ax::RightZ),
            nec::AXIS_DPADX => Axis(Ax::DPadX),
            nec::AXIS_DPADY => Axis(Ax::DPadY)
        ];

        // Remove all mappings that don't have corresponding element in gamepad. Partial fix to #83
        let axes = [
            nec::AXIS_DPADX,
            nec::AXIS_DPADY,
            nec::AXIS_LEFTZ,
            nec::AXIS_LSTICKX,
            nec::AXIS_LSTICKY,
            nec::AXIS_RSTICKX,
            nec::AXIS_RSTICKY,
            nec::AXIS_LT,
            nec::AXIS_LT2,
            nec::AXIS_RT,
            nec::AXIS_RT2,
            nec::AXIS_RIGHTZ,
        ];
        let btns = [
            nec::BTN_SOUTH,
            nec::BTN_NORTH,
            nec::BTN_WEST,
            nec::BTN_WEST,
            nec::BTN_C,
            nec::BTN_Z,
            nec::BTN_LT,
            nec::BTN_LT2,
            nec::BTN_RT,
            nec::BTN_RT2,
            nec::BTN_SELECT,
            nec::BTN_START,
            nec::BTN_MODE,
            nec::BTN_LTHUMB,
            nec::BTN_RTHUMB,
            nec::BTN_DPAD_DOWN,
            nec::BTN_DPAD_LEFT,
            nec::BTN_DPAD_RIGHT,
            nec::BTN_DPAD_UP,
        ];

        for axis in &axes {
            if !gamepad.axes().contains(axis) {
                mappings.remove(axis);
            }
        }

        for btn in &btns {
            if !gamepad.buttons().contains(btn) {
                mappings.remove(btn);
            }
        }

        Mapping {
            mappings,
            name: String::new(),
            default: true,
            hats_mapped: 0,
            #[cfg(target_os = "linux")]
            digital_hats: FnvHashMap::default(),
            #[cfg(target_os = "linux")]
            automatic: false,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn from_data(
        data: &MappingData,
        buttons: &[EvCode],
        axes: &[EvCode],
        name: &str,
        uuid: Uuid,
    ) -> Result<(Self, String), MappingError> {
        use crate::constants::*;

        if !Self::is_name_valid(name) {
            return Err(MappingError::InvalidName);
        }

        let mut mappings = FnvHashMap::default();
        let mut sdl_mappings = format!("{},{},", uuid.as_simple(), name);

        {
            let mut add_button = |ident, ev_code, mapped_btn| {
                Self::add_button(
                    ident,
                    ev_code,
                    mapped_btn,
                    buttons,
                    &mut sdl_mappings,
                    &mut mappings,
                )
            };

            for (button, &ev_code) in &data.buttons {
                match button as u16 {
                    BTN_SOUTH => add_button("a", ev_code, Button::South)?,
                    BTN_EAST => add_button("b", ev_code, Button::East)?,
                    BTN_WEST => add_button("x", ev_code, Button::West)?,
                    BTN_NORTH => add_button("y", ev_code, Button::North)?,
                    BTN_LT => add_button("leftshoulder", ev_code, Button::LeftTrigger)?,
                    BTN_RT => add_button("rightshoulder", ev_code, Button::RightTrigger)?,
                    BTN_LT2 => add_button("lefttrigger", ev_code, Button::LeftTrigger2)?,
                    BTN_RT2 => add_button("righttrigger", ev_code, Button::RightTrigger2)?,
                    BTN_SELECT => add_button("back", ev_code, Button::Select)?,
                    BTN_START => add_button("start", ev_code, Button::Start)?,
                    BTN_MODE => add_button("guide", ev_code, Button::Mode)?,
                    BTN_LTHUMB => add_button("leftstick", ev_code, Button::LeftThumb)?,
                    BTN_RTHUMB => add_button("rightstick", ev_code, Button::RightThumb)?,
                    BTN_DPAD_UP => add_button("dpup", ev_code, Button::DPadUp)?,
                    BTN_DPAD_DOWN => add_button("dpdown", ev_code, Button::DPadDown)?,
                    BTN_DPAD_LEFT => add_button("dpleft", ev_code, Button::DPadLeft)?,
                    BTN_DPAD_RIGHT => add_button("dpright", ev_code, Button::DPadRight)?,
                    BTN_C => add_button("c", ev_code, Button::C)?,
                    BTN_Z => add_button("z", ev_code, Button::Z)?,
                    BTN_UNKNOWN => return Err(MappingError::UnknownElement),
                    _ => unreachable!(),
                }
            }
        }

        {
            let mut add_axis = |ident, ev_code, mapped_axis| {
                Self::add_axis(
                    ident,
                    ev_code,
                    mapped_axis,
                    axes,
                    &mut sdl_mappings,
                    &mut mappings,
                )
            };

            for (axis, &ev_code) in &data.axes {
                match axis as u16 {
                    AXIS_LSTICKX => add_axis("leftx", ev_code, Axis::LeftStickX)?,
                    AXIS_LSTICKY => add_axis("lefty", ev_code, Axis::LeftStickY)?,
                    AXIS_RSTICKX => add_axis("rightx", ev_code, Axis::RightStickX)?,
                    AXIS_RSTICKY => add_axis("righty", ev_code, Axis::RightStickY)?,
                    AXIS_LEFTZ => add_axis("leftz", ev_code, Axis::LeftZ)?,
                    AXIS_RIGHTZ => add_axis("rightz", ev_code, Axis::RightZ)?,
                    AXIS_UNKNOWN => return Err(MappingError::UnknownElement),
                    _ => unreachable!(),
                }
            }
        }

        let mapping = Mapping {
            mappings,
            name: name.to_owned(),
            default: false,
            hats_mapped: 0,
            #[cfg(target_os = "linux")]
            digital_hats: FnvHashMap::default(),
            #[cfg(target_os = "linux")]
            automatic: false,
        };

        Ok((mapping, sdl_mappings))
    }

    pub fn parse_sdl_mapping(
        line: &str,
        buttons: &[EvCode],
        axes: &[EvCode],
    ) -> Result<Self, ParseSdlMappingError> {
        let mut mapping = Mapping::new();
        let mut parser = Parser::new(line);

        let mut uuid: Option<Uuid> = None;
        while let Some(token) = parser.next_token() {
            if let Err(ref e) = token {
                if e.kind() == &ParserErrorKind::EmptyValue {
                    continue;
                }
            }

            let token = token?;

            match token {
                Token::Platform(platform) => {
                    if platform != SDL_PLATFORM_NAME {
                        warn!("Mappings for different platform – {}", platform);
                    }
                }
                Token::Uuid(v) => uuid = Some(v),

                Token::Name(name) => mapping.name = name.to_owned(),
                Token::AxisMapping { from, to, .. } => {
                    let axis = axes.get(from as usize).cloned();
                    if let Some(axis) = axis {
                        mapping.mappings.insert(axis, to);
                    } else {
                        warn!(
                            "SDL-mapping {} {}: Unknown axis a{}",
                            uuid.unwrap(),
                            mapping.name,
                            from
                        )
                    }
                }
                Token::ButtonMapping { from, to, .. } => {
                    let btn = buttons.get(from as usize).cloned();

                    if let Some(btn) = btn {
                        mapping.mappings.insert(btn, to);
                    } else {
                        warn!(
                            "SDL-mapping {} {}: Unknown button b{}",
                            uuid.unwrap(),
                            mapping.name,
                            from
                        )
                    }
                }
                Token::HatMapping {
                    hat, direction, to, ..
                } => {
                    if hat != 0 {
                        warn!(
                            "Hat mappings are only supported for dpads (requested to map hat \
                             {}.{} to {:?}",
                            hat, direction, to
                        );
                    } else {
                        // We  don't have anything like "hat" in gilrs, so let's jus assume that
                        // user want to map dpad axes.
                        //
                        // We have to add mappings for axes AND buttons, because axis_dpad_to_button
                        // filter may transform event to button event.
                        let (from_axis, from_btn) = match direction {
                            1 => (nec::AXIS_DPADY, nec::BTN_DPAD_UP),
                            4 => (nec::AXIS_DPADY, nec::BTN_DPAD_DOWN),
                            2 => (nec::AXIS_DPADX, nec::BTN_DPAD_RIGHT),
                            8 => (nec::AXIS_DPADX, nec::BTN_DPAD_LEFT),
                            0 => continue, // FIXME: I have no idea what 0 means here
                            _ => return Err(ParseSdlMappingError::UnknownHatDirection),
                        };

                        if to.is_button() {
                            match to {
                                AxisOrBtn::Btn(Button::DPadLeft | Button::DPadRight) => {
                                    mapping
                                        .mappings
                                        .insert(from_axis, AxisOrBtn::Axis(Axis::DPadX));
                                }
                                AxisOrBtn::Btn(Button::DPadUp | Button::DPadDown) => {
                                    mapping
                                        .mappings
                                        .insert(from_axis, AxisOrBtn::Axis(Axis::DPadY));
                                }
                                _ => (),
                            }
                            mapping.mappings.insert(from_btn, to);
                        } else {
                            mapping.mappings.insert(from_axis, to);
                        }

                        mapping.hats_mapped |= direction as u8;
                    }
                }
            }
        }

        Ok(mapping)
    }

    fn add_button(
        ident: &str,
        ev_code: EvCode,
        mapped_btn: Button,
        buttons: &[EvCode],
        sdl_mappings: &mut String,
        mappings: &mut FnvHashMap<EvCode, AxisOrBtn>,
    ) -> Result<(), MappingError> {
        let n_btn = buttons
            .iter()
            .position(|&x| x == ev_code)
            .ok_or(MappingError::InvalidCode(ev::Code(ev_code)))?;
        let _ = write!(sdl_mappings, "{}:b{},", ident, n_btn);
        mappings.insert(ev_code, AxisOrBtn::Btn(mapped_btn));
        Ok(())
    }

    fn add_axis(
        ident: &str,
        ev_code: EvCode,
        mapped_axis: Axis,
        axes: &[EvCode],
        sdl_mappings: &mut String,
        mappings: &mut FnvHashMap<EvCode, AxisOrBtn>,
    ) -> Result<(), MappingError> {
        let n_axis = axes
            .iter()
            .position(|&x| x == ev_code)
            .ok_or(MappingError::InvalidCode(ev::Code(ev_code)))?;
        let _ = write!(sdl_mappings, "{}:a{},", ident, n_axis);
        mappings.insert(ev_code, AxisOrBtn::Axis(mapped_axis));
        Ok(())
    }

    fn is_name_valid(name: &str) -> bool {
        !name.chars().any(|x| x == ',')
    }

    pub fn map(&self, code: &EvCode) -> Option<AxisOrBtn> {
        self.mappings.get(code).cloned()
    }

    pub fn map_rev(&self, el: &AxisOrBtn) -> Option<EvCode> {
        self.mappings.iter().find(|x| x.1 == el).map(|x| *x.0)
    }

    pub fn is_default(&self) -> bool {
        self.default
    }

    /// Return bit field with mapped hats. Only for mappings created from SDL format this function
    /// can return non-zero value.
    pub fn hats_mapped(&self) -> u8 {
        self.hats_mapped
    }

    /// Project Avalanche patch T23: whether this is SDL's automatic mapping (`select`).
    #[cfg(target_os = "linux")]
    pub fn is_automatic(&self) -> bool {
        self.automatic
    }

    /// Project Avalanche patch T23: for an axis the automatic mapping reads as a digital hat, the raw `value` as SDL
    /// reads it — for a d-pad axis the hat's position −1, 0 or +1, for a hat direction bound to a button 1.0 when the
    /// hat points that way, else 0.0; `None` for every other axis (read as gilrs reads it).
    #[cfg(target_os = "linux")]
    pub(crate) fn digital_hat_value(&mut self, code: EvCode, value: i32) -> Option<f32> {
        let hat = self.digital_hats.get_mut(&code)?;
        let position = hat.position(value);
        Some(match hat.press {
            Some(side) => f32::from(u8::from(position == side)),
            None => f32::from(position),
        })
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ParseSdlMappingError {
    UnknownHatDirection,
    ParseError(ParserError),
}

impl From<ParserError> for ParseSdlMappingError {
    fn from(f: ParserError) -> Self {
        ParseSdlMappingError::ParseError(f)
    }
}

impl Error for ParseSdlMappingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        if let ParseSdlMappingError::ParseError(ref err) = self {
            Some(err)
        } else {
            None
        }
    }
}

impl Display for ParseSdlMappingError {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> FmtResult {
        match self {
            ParseSdlMappingError::UnknownHatDirection => {
                fmt.write_str("hat direction wasn't 1, 2, 4 or 8")
            }
            ParseSdlMappingError::ParseError(_) => fmt.write_str("parsing error"),
        }
    }
}

#[derive(Debug)]
pub struct MappingDb {
    mappings: HashMap<Uuid, String>,
    // Project Avalanche patch T22: each row's origin, and the GUIDs in SDL's list order (`s_pSupportedGamepads`: a new
    // GUID is appended, a replaced row keeps its place; SDL 3.4.20 `src/joystick/SDL_gamepad.c:2199-2246`).
    #[cfg(target_os = "linux")]
    origins: HashMap<Uuid, Origin>,
    #[cfg(target_os = "linux")]
    order: Vec<Uuid>,
}

/// Project Avalanche patch T22: where a row came from, in SDL's priority order (`SDL_GamepadMappingPriority`:
/// DEFAULT < API < USER, `src/joystick/SDL_gamepad.c:92-97`): the bundled database, `GilrsBuilder::add_mappings`, the
/// `SDL_GAMECONTROLLERCONFIG` environment variable (SDL's hint of that name).
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Origin {
    Database,
    Application,
    Environment,
}

/// Project Avalanche patch T22: a row [`MappingDb::lookup`] found: its text, origin and GUID, and whether it matched
/// only without the version.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
pub(crate) struct Found<'a> {
    pub(crate) row: &'a str,
    pub(crate) origin: Origin,
    pub(crate) guid: Uuid,
    pub(crate) without_version: bool,
}

impl MappingDb {
    pub fn new() -> Self {
        MappingDb {
            mappings: HashMap::new(),
            #[cfg(target_os = "linux")]
            origins: HashMap::new(),
            #[cfg(target_os = "linux")]
            order: Vec::new(),
        }
    }

    pub fn add_included_mappings(&mut self) {
        let rows = include_str!(concat!(env!("OUT_DIR"), "/gamecontrollerdb.txt"));
        // Project Avalanche patch T22: the bundled rows rank as SDL's DEFAULT mappings.
        #[cfg(target_os = "linux")]
        self.insert_from(rows, Origin::Database);
        #[cfg(not(target_os = "linux"))]
        self.insert(rows);
    }

    pub fn add_env_mappings(&mut self) {
        if let Ok(mapping) = env::var("SDL_GAMECONTROLLERCONFIG") {
            // Project Avalanche patch T22: the variable's rows rank as SDL's USER mappings (its hint).
            #[cfg(target_os = "linux")]
            self.insert_from(&mapping, Origin::Environment);
            #[cfg(not(target_os = "linux"))]
            self.insert(&mapping);
        }
    }

    // Project Avalanche patch T22: rows the application adds rank as SDL's API mappings.
    #[cfg(target_os = "linux")]
    pub fn insert(&mut self, s: &str) {
        self.insert_from(s, Origin::Application);
    }

    /// Project Avalanche patch T22: `insert` as SDL's `SDL_PrivateAddMappingForGUID` keeps rows (SDL 3.4.20
    /// `src/joystick/SDL_gamepad.c:2199-2246`): a row replaces the one with its GUID only at the same or a higher
    /// priority, in that row's place; a new GUID goes to the end.
    #[cfg(target_os = "linux")]
    fn insert_from(&mut self, s: &str, origin: Origin) {
        for mapping in s.lines() {
            let pat = "platform:";
            if let Some(offset) = mapping.find(pat).map(|o| o + pat.len()) {
                let s = &mapping[offset..];
                let end = s.find(',').unwrap_or(s.len());

                if &s[..end] != SDL_PLATFORM_NAME {
                    continue;
                }
            }

            let Some(uuid) = mapping
                .split(',')
                .next()
                .and_then(|s| Uuid::parse_str(s).ok())
            else {
                continue;
            };
            match self.origins.get(&uuid) {
                Some(&existing) if existing > origin => continue,
                Some(_) => {}
                None => self.order.push(uuid),
            }
            self.origins.insert(uuid, origin);
            self.mappings.insert(uuid, mapping.to_owned());
        }
    }

    /// Project Avalanche patch T22: SDL's lookup of a device's row (`SDL_PrivateGetGamepadMappingForGUID`, SDL 3.4.20
    /// `src/joystick/SDL_gamepad.c:1426-1462` with `SDL_PrivateMatchGamepadMappingForGUID` 1372-1421): the exact GUID;
    /// else, for a GUID that names a vendor and a product (`SDL_JoystickGUIDUsesVersion`, `SDL_joystick.c:3132-3146`),
    /// the first row in SDL's list order whose GUID equals it with both versions cleared. No Linux row of the bundled
    /// database carries SDL's `crc:` field and gilrs' GUIDs carry no name CRC, so SDL's CRC passes are these two.
    #[cfg(target_os = "linux")]
    pub(crate) fn lookup(&self, uuid: Uuid) -> Option<Found<'_>> {
        let found = |guid: Uuid, without_version: bool| {
            Some(Found {
                row: self.mappings.get(&guid)?,
                origin: *self.origins.get(&guid)?,
                guid,
                without_version,
            })
        };
        if self.mappings.contains_key(&uuid) {
            return found(uuid, false);
        }
        if !sdl_guid::uses_version(uuid.as_bytes()) {
            return None;
        }
        let want = sdl_guid::without_version(uuid.as_bytes());
        let guid = *self
            .order
            .iter()
            .find(|g| !g.is_nil() && sdl_guid::without_version(g.as_bytes()) == want)?;
        found(guid, true)
    }

    #[cfg(not(target_os = "linux"))]
    pub fn insert(&mut self, s: &str) {
        for mapping in s.lines() {
            let pat = "platform:";
            if let Some(offset) = mapping.find(pat).map(|o| o + pat.len()) {
                let s = &mapping[offset..];
                let end = s.find(',').unwrap_or(s.len());

                if &s[..end] != SDL_PLATFORM_NAME {
                    continue;
                }
            }

            mapping
                .split(',')
                .next()
                .and_then(|s| Uuid::parse_str(s).ok())
                .and_then(|uuid| self.mappings.insert(uuid, mapping.to_owned()));
        }
    }

    // Project Avalanche patch T22: on Linux the mapping is looked up in SDL's order (`lookup`); the exact row stays for
    // the other platforms and the tests.
    #[cfg(any(not(target_os = "linux"), test))]
    pub fn get(&self, uuid: Uuid) -> Option<&str> {
        self.mappings.get(&uuid).map(String::as_ref)
    }

    pub fn len(&self) -> usize {
        self.mappings.len()
    }
}

/// Stores data used to map gamepad buttons and axes.
///
/// After you add all mappings, use
/// [`Gamepad::set_mapping(…)`](struct.Gamepad.html#method.set_mapping) to change mapping of
/// existing gamepad.
///
/// See `examples/mapping.rs` for more detailed example.
#[derive(Debug, Clone, Default)]
// Re-exported as Mapping
pub struct MappingData {
    buttons: VecMap<EvCode>,
    axes: VecMap<EvCode>,
}

impl MappingData {
    /// Creates new `Mapping`.
    pub fn new() -> Self {
        MappingData {
            buttons: VecMap::with_capacity(18),
            axes: VecMap::with_capacity(11),
        }
    }

    /// Returns `EvCode` associated with button index.
    pub fn button(&self, idx: Button) -> Option<ev::Code> {
        self.buttons.get(idx as usize).cloned().map(ev::Code)
    }

    /// Returns `EvCode` associated with axis index.
    pub fn axis(&self, idx: Axis) -> Option<ev::Code> {
        self.axes.get(idx as usize).cloned().map(ev::Code)
    }

    /// Inserts new button mapping.
    pub fn insert_btn(&mut self, from: ev::Code, to: Button) -> Option<ev::Code> {
        self.buttons.insert(to as usize, from.0).map(ev::Code)
    }

    /// Inserts new axis mapping.
    pub fn insert_axis(&mut self, from: ev::Code, to: Axis) -> Option<ev::Code> {
        self.axes.insert(to as usize, from.0).map(ev::Code)
    }

    /// Removes button and returns associated `NativEvCode`.
    pub fn remove_button(&mut self, idx: Button) -> Option<ev::Code> {
        self.buttons.remove(idx as usize).map(ev::Code)
    }

    /// Removes axis and returns associated `NativEvCode`.
    pub fn remove_axis(&mut self, idx: Axis) -> Option<ev::Code> {
        self.axes.remove(idx as usize).map(ev::Code)
    }
}

/// The error type for functions related to gamepad mapping.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MappingError {
    /// Gamepad does not have element referenced by `EvCode`.
    InvalidCode(ev::Code),
    /// Name contains comma (',').
    InvalidName,
    /// This function is not implemented for current platform.
    NotImplemented,
    /// Gamepad is not connected.
    NotConnected,
    /// Same gamepad element is referenced by axis and button.
    DuplicatedEntry,
    /// `Mapping` with `Button::Unknown` or `Axis::Unknown`.
    UnknownElement,
    /// `Mapping` have button or axis that are not present in SDL2.
    NotSdl2Compatible,
}

impl Error for MappingError {}

impl Display for MappingError {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        let sbuf;
        let s = match self {
            MappingError::InvalidCode(code) => {
                sbuf = format!("gamepad does not have element with {}", code);
                sbuf.as_ref()
            }
            MappingError::InvalidName => "name can not contain comma",
            MappingError::NotImplemented => {
                "current platform does not implement setting custom mappings"
            }
            MappingError::NotConnected => "gamepad is not connected",
            MappingError::DuplicatedEntry => {
                "same gamepad element is referenced by axis and button"
            }
            MappingError::UnknownElement => "Button::Unknown and Axis::Unknown are not allowed",
            MappingError::NotSdl2Compatible => "one of buttons or axes is not compatible with SDL2",
        };

        f.write_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ev::{Axis, Button};
    use gilrs_core::native_ev_codes as nec;
    use gilrs_core::EvCode;
    use uuid::Uuid;
    // Do not include platform, mapping from (with UUID modified)
    // https://github.com/gabomdq/SDL_GameControllerDB/blob/master/gamecontrollerdb.txt
    const TEST_STR: &str = "03000000260900008888000000010001,GameCube {WiseGroup USB \
                            box},a:b0,b:b2,y:b3,x:b1,start:b7,rightshoulder:b6,dpup:h0.1,dpleft:\
                            h0.8,dpdown:h0.4,dpright:h0.2,leftx:a0,lefty:a1,rightx:a2,righty:a3,\
                            lefttrigger:a4,righttrigger:a5,";

    const BUTTONS: [EvCode; 15] = [
        nec::BTN_SOUTH,
        nec::BTN_EAST,
        nec::BTN_C,
        nec::BTN_NORTH,
        nec::BTN_WEST,
        nec::BTN_Z,
        nec::BTN_LT,
        nec::BTN_RT,
        nec::BTN_LT2,
        nec::BTN_RT2,
        nec::BTN_SELECT,
        nec::BTN_START,
        nec::BTN_MODE,
        nec::BTN_LTHUMB,
        nec::BTN_RTHUMB,
    ];

    const AXES: [EvCode; 12] = [
        nec::AXIS_LSTICKX,
        nec::AXIS_LSTICKY,
        nec::AXIS_LEFTZ,
        nec::AXIS_RSTICKX,
        nec::AXIS_RSTICKY,
        nec::AXIS_RIGHTZ,
        nec::AXIS_DPADX,
        nec::AXIS_DPADY,
        nec::AXIS_RT,
        nec::AXIS_LT,
        nec::AXIS_RT2,
        nec::AXIS_LT2,
    ];

    #[test]
    fn mapping() {
        Mapping::parse_sdl_mapping(TEST_STR, &BUTTONS, &AXES).unwrap();
    }

    #[test]
    fn from_data() {
        let uuid = Uuid::nil();
        let name = "Best Gamepad";
        let buttons = BUTTONS.iter().cloned().map(ev::Code).collect::<Vec<_>>();
        let axes = AXES.iter().cloned().map(ev::Code).collect::<Vec<_>>();

        let mut data = MappingData::new();
        data.insert_axis(axes[0], Axis::LeftStickX);
        data.insert_axis(axes[1], Axis::LeftStickY);
        data.insert_axis(axes[2], Axis::LeftZ);
        data.insert_axis(axes[3], Axis::RightStickX);
        data.insert_axis(axes[4], Axis::RightStickY);
        data.insert_axis(axes[5], Axis::RightZ);

        data.insert_btn(buttons[0], Button::South);
        data.insert_btn(buttons[1], Button::East);
        data.insert_btn(buttons[3], Button::North);
        data.insert_btn(buttons[4], Button::West);
        data.insert_btn(buttons[5], Button::Select);
        data.insert_btn(buttons[6], Button::Start);
        data.insert_btn(buttons[7], Button::DPadDown);
        data.insert_btn(buttons[8], Button::DPadLeft);
        data.insert_btn(buttons[9], Button::RightThumb);

        let (mappings, sdl_mappings) =
            Mapping::from_data(&data, &BUTTONS, &AXES, name, uuid).unwrap();
        let sdl_mappings = Mapping::parse_sdl_mapping(&sdl_mappings, &BUTTONS, &AXES).unwrap();
        assert_eq!(mappings, sdl_mappings);

        let incorrect_mappings = Mapping::from_data(&data, &BUTTONS, &AXES, "Inval,id name", uuid);
        assert_eq!(Err(MappingError::InvalidName), incorrect_mappings);

        data.insert_btn(ev::Code(nec::BTN_DPAD_RIGHT), Button::DPadRight);
        let incorrect_mappings = Mapping::from_data(&data, &BUTTONS, &AXES, name, uuid);
        assert_eq!(
            Err(MappingError::InvalidCode(ev::Code(nec::BTN_DPAD_RIGHT))),
            incorrect_mappings
        );

        data.insert_btn(ev::Code(BUTTONS[3]), Button::Unknown);
        let incorrect_mappings = Mapping::from_data(&data, &BUTTONS, &AXES, name, uuid);
        assert_eq!(Err(MappingError::UnknownElement), incorrect_mappings);
    }

    #[test]
    fn with_mappings() {
        let mappings = format!(
            "\nShould be ignored\nThis also should,be ignored\n\n{}",
            TEST_STR
        );
        let mut db = MappingDb::new();
        db.add_included_mappings();
        db.insert(&mappings);

        assert_eq!(
            Some(TEST_STR),
            db.get(Uuid::parse_str("03000000260900008888000000010001").unwrap())
        );
    }

    // Project Avalanche patch T19: exercise the bundled mapping with this xpad device's native-code order.
    #[cfg(target_os = "linux")]
    #[test]
    fn nacon_gc100_0110_bundled_mapping_normalizes_the_complete_xpad_layout() {
        let mut db = MappingDb::new();
        db.add_included_mappings();
        let guid = Uuid::parse_str("03000000853200000706000010010000").unwrap();
        let row = db
            .get(guid)
            .expect("the physical Nacon GC-100 0110 is bundled");
        let buttons = [
            nec::BTN_SOUTH,
            nec::BTN_EAST,
            nec::BTN_NORTH,
            nec::BTN_WEST,
            nec::BTN_LT,
            nec::BTN_RT,
            nec::BTN_SELECT,
            nec::BTN_START,
            nec::BTN_MODE,
            nec::BTN_LTHUMB,
            nec::BTN_RTHUMB,
        ];
        let mapping = Mapping::parse_sdl_mapping(row, &buttons, &AXES[..8]).unwrap();
        for (code, button) in [
            (nec::BTN_SOUTH, Button::South),
            (nec::BTN_EAST, Button::East),
            (nec::BTN_NORTH, Button::West),
            (nec::BTN_WEST, Button::North),
            (nec::BTN_LT, Button::LeftTrigger),
            (nec::BTN_RT, Button::RightTrigger),
            (nec::BTN_SELECT, Button::Select),
            (nec::BTN_START, Button::Start),
            (nec::BTN_MODE, Button::Mode),
            (nec::BTN_LTHUMB, Button::LeftThumb),
            (nec::BTN_RTHUMB, Button::RightThumb),
            (nec::AXIS_LEFTZ, Button::LeftTrigger2),
            (nec::AXIS_RIGHTZ, Button::RightTrigger2),
            (nec::BTN_DPAD_UP, Button::DPadUp),
            (nec::BTN_DPAD_DOWN, Button::DPadDown),
            (nec::BTN_DPAD_LEFT, Button::DPadLeft),
            (nec::BTN_DPAD_RIGHT, Button::DPadRight),
        ] {
            assert_eq!(mapping.map(&code), Some(AxisOrBtn::Btn(button)));
            assert_eq!(mapping.map_rev(&AxisOrBtn::Btn(button)), Some(code));
        }
        for (code, axis) in [
            (nec::AXIS_LSTICKX, Axis::LeftStickX),
            (nec::AXIS_LSTICKY, Axis::LeftStickY),
            (nec::AXIS_RSTICKX, Axis::RightStickX),
            (nec::AXIS_RSTICKY, Axis::RightStickY),
            (nec::AXIS_DPADX, Axis::DPadX),
            (nec::AXIS_DPADY, Axis::DPadY),
        ] {
            assert_eq!(mapping.map(&code), Some(AxisOrBtn::Axis(axis)));
        }
        assert_eq!(mapping.map_rev(&AxisOrBtn::Axis(Axis::LeftZ)), None);
        assert_eq!(mapping.map_rev(&AxisOrBtn::Axis(Axis::RightZ)), None);
        assert_eq!(mapping.mappings.len(), 23);
        assert_eq!(mapping.hats_mapped(), 0x0f);
        assert!(!mapping.is_default());
    }

    // Project Avalanche patch T19: neither a nearby device GUID nor a different platform is a fallback match.
    #[cfg(target_os = "linux")]
    #[test]
    fn nacon_gc100_patch_keeps_exact_guid_and_platform_scope() {
        let mut db = MappingDb::new();
        db.add_included_mappings();
        let guid = Uuid::parse_str("03000000853200000706000010010000").unwrap();
        let existing = Uuid::parse_str("03000000853200000706000012010000").unwrap();
        let added = db.get(guid).unwrap().to_owned();
        let prior = db.get(existing).expect("the original 0112 entry remains");
        assert_eq!(
            added.split_once(',').unwrap().1,
            prior.split_once(',').unwrap().1
        );
        assert_eq!(
            db.get(Uuid::parse_str("03000000853200000706000013010000").unwrap()),
            None
        );
        assert_eq!(
            db.get(Uuid::parse_str("03000000853200000806000010010000").unwrap()),
            None
        );
        db.insert("03000000853200000706000010010000,Not Linux,lefttrigger:b0,platform:Windows,");
        assert_eq!(db.get(guid), Some(added.as_str()));
    }

    // Project Avalanche patch T22: SDL's lookup order and row priorities.
    #[cfg(target_os = "linux")]
    #[test]
    fn lookup_matches_sdl_exact_then_without_version_in_list_order() {
        let guid = |s: &str| Uuid::parse_str(s).unwrap();
        let mut db = MappingDb::new();
        db.insert_from(
            "03000000341200007856000000020000,Pad v0200,a:b0,platform:Linux,\n\
             03000000341200007856000000010000,Pad v0100,a:b1,platform:Linux,\n\
             03000000341200007856000000030000,Pad v0300 Windows,a:b2,platform:Windows,\n\
             03000000000000007856000000010000,No vendor,a:b3,platform:Linux,",
            Origin::Database,
        );
        let exact = db.lookup(guid("03000000341200007856000000010000")).unwrap();
        assert!(exact.row.contains("Pad v0100") && !exact.without_version);
        // No exact row: the first row in list order with the same GUID but the version.
        let other = db.lookup(guid("03000000341200007856000010010000")).unwrap();
        assert!(other.row.contains("Pad v0200") && other.without_version);
        assert_eq!(other.guid, guid("03000000341200007856000000020000"));
        // A row for another platform is not in the list.
        assert!(
            db.lookup(guid("03000000341200007856000000030000"))
                .unwrap()
                .without_version
        );
        // Without a vendor (or a product) SDL does not drop the version.
        assert!(db
            .lookup(guid("03000000000000007856000000020000"))
            .is_none());
        assert!(db
            .lookup(guid("03000000341200000000000000010000"))
            .is_none());
        // Another bus is another device.
        assert!(db
            .lookup(guid("05000000341200007856000000010000"))
            .is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn rows_replace_by_sdls_priority_and_keep_their_place() {
        let id = "03000000341200007856000000010000";
        let first = Uuid::parse_str(id).unwrap();
        let mut db = MappingDb::new();
        db.insert(&format!("{id},From the application,a:b0,"));
        db.insert_from(
            &format!("{id},From the database,a:b1,\n03000000341200007856000000020000,Later,a:b2,"),
            Origin::Database,
        );
        let found = db.lookup(first).unwrap();
        assert_eq!(
            (found.origin, found.row.contains("application")),
            (Origin::Application, true)
        );
        db.insert_from(
            &format!("{id},From the environment,a:b3,"),
            Origin::Environment,
        );
        let found = db.lookup(first).unwrap();
        assert_eq!(
            (found.origin, found.row.contains("environment")),
            (Origin::Environment, true)
        );
        db.insert(&format!("{id},Application again,a:b4,"));
        assert_eq!(db.lookup(first).unwrap().origin, Origin::Environment);
        // The replaced row keeps its place before the later one.
        let without_version = db
            .lookup(Uuid::parse_str("03000000341200007856000000090000").unwrap())
            .unwrap();
        assert_eq!(without_version.guid, first);
        assert_eq!(db.len(), 2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sdl_guid_reading() {
        let g = |s: &str| *Uuid::parse_str(s).unwrap().as_bytes();
        assert_eq!(
            sdl_guid::vendor(&g("030000004c050000cc09000011810000")),
            0x054c
        );
        assert!(sdl_guid::uses_version(&g(
            "030000004c050000cc09000011810000"
        )));
        // Bus 0x20 and up (other than 0xff) is not SDL's standard form.
        assert_eq!(sdl_guid::vendor(&g("200000004c050000cc09000011810000")), 0);
        assert_eq!(
            sdl_guid::vendor(&g("ff0000004c050000cc09000011810000")),
            0x054c
        );
        // An MFi GUID's version bits are its buttons.
        assert!(!sdl_guid::uses_version(&g(
            "030000004c050000cc09000011816d00"
        )));
        assert_eq!(
            sdl_guid::without_version(&g("030000004c050000cc09000011810000")),
            g("030000004c050000cc09000000000000")
        );
    }

    // Project Avalanche patch T24: every Linux row of the bundled database (SDL_GameControllerDB c0f11d40 with T19's
    // row) is kept by the database and read by the row parser.
    #[cfg(target_os = "linux")]
    #[test]
    fn every_bundled_linux_row_is_read() {
        let rows: Vec<&str> = include_str!(concat!(env!("OUT_DIR"), "/gamecontrollerdb.txt"))
            .lines()
            .collect();
        let mut db = MappingDb::new();
        db.add_included_mappings();
        let buttons: Vec<EvCode> = (0..64)
            .map(|i| EvCode::from_evdev(0x01, 0x120 + i))
            .collect();
        let axes: Vec<EvCode> = (0..64).map(|i| EvCode::from_evdev(0x03, i)).collect();
        let read = rows
            .iter()
            .filter(|row| Mapping::parse_sdl_mapping(row, &buttons, &axes).is_ok())
            .count();
        // SDL's "xinput" row names no device GUID (SDL applies it to XInput devices, which only Windows has).
        let named: Vec<&&str> = rows
            .iter()
            .filter(|row| {
                row.split(',')
                    .next()
                    .and_then(|g| Uuid::parse_str(g).ok())
                    .is_some()
            })
            .collect();
        println!(
            "bundled Linux rows: {}, with a GUID: {}, in the database: {}, read: {read}",
            rows.len(),
            named.len(),
            db.len()
        );
        assert_eq!((db.len(), read), (named.len(), rows.len()));
        assert_eq!((rows.len(), named.len()), (749, 748));
        assert!(rows.iter().any(|row| row.starts_with("xinput,")));
    }
}
