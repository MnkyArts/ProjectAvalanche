// Project Avalanche patch T26 (Engine/vendor/PATCHES.md): the gamepads SDL 3.4.20 does not open because of the
// environment a launcher sets, Steam among them: `SDL_ShouldIgnoreGamepad` (src/joystick/SDL_gamepad.c:3260-3319,
// after its name words) with its two lists (`SDL_LoadVIDPIDList`, src/joystick/SDL_joystick.c:3975-4003, called once
// by `SDL_InitGamepadMappings`, SDL_gamepad.c:3017-3018; `SDL_LoadVIDPIDListFromHint`, 3876-3915;
// `SDL_VIDPIDInList`, 4005-4021). Steam Input reads a physical pad itself and offers it again as its virtual pad; it names the physical
// pads in these variables so that a game does not read them twice (Valve's "Getting Started for Developers": SDL
// 2.0.8 or later "allows Steam to tell SDL to ignore input for controllers configured by Steam Input, which avoids
// double-input bugs").

use std::ffi::{OsStr, OsString};
use std::fmt::{self, Display, Formatter, Write as _};
use std::fs::File;
use std::io::Read;
use std::os::raw::c_long;
use std::path::Path;

/// `SDL_HINT_GAMECONTROLLER_IGNORE_DEVICES`: the pads not to open.
pub(crate) const IGNORE_DEVICES: &str = "SDL_GAMECONTROLLER_IGNORE_DEVICES";
/// `SDL_HINT_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT`: when it names any pad, the only pads to open.
pub(crate) const IGNORE_DEVICES_EXCEPT: &str = "SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT";
/// Whether Steam's virtual gamepad is opened (read with `getenv`, SDL_gamepad.c:3292).
pub(crate) const ALLOW_STEAM_VIRTUAL_GAMEPAD: &str = "SDL_GAMECONTROLLER_ALLOW_STEAM_VIRTUAL_GAMEPAD";

// PORT(choice): a list longer than 64 KiB (the variable's value, or the file an "@" names) is not read and the log
// says so; only a regular file is read (not a pipe or a device, which could block or never end). SDL 3.4.20 reads the
// whole value or file (SDL_LoadFile) with no limit; Steam's list names one entry of 13 bytes per pad. The console read
// XInput pads only and had no such list.
pub(crate) const LIST_LIMIT: usize = 64 * 1024;

/// `SDL_IsJoystickSteamVirtualGamepad` (SDL_joystick.c:3272-3279): Valve's vendor and the Steam virtual gamepad's
/// product (`USB_VENDOR_VALVE`, `USB_PRODUCT_STEAM_VIRTUAL_GAMEPAD`, src/joystick/usb_ids.h:68, 208).
#[cfg(not(target_os = "macos"))]
fn is_steam_virtual_gamepad(vendor: u16, product: u16, _version: u16) -> bool {
    (vendor, product) == (0x28de, 0x11ff)
}

/// `SDL_IsJoystickSteamVirtualGamepad` on macOS: an Xbox 360 wired controller with version 0
/// (`USB_VENDOR_MICROSOFT`, `USB_PRODUCT_XBOX360_WIRED_CONTROLLER`, usb_ids.h:48, 190).
#[cfg(target_os = "macos")]
fn is_steam_virtual_gamepad(vendor: u16, product: u16, version: u16) -> bool {
    (vendor, product) == (0x045e, 0x028e) && version == 0
}

/// What a list variable held.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum List {
    NotSet,
    /// Its entries, `vendor << 16 | product`, in the order written (none when nothing in it reads as one).
    Read(Vec<u32>),
    /// It could not be read (the file) or was not (the limit): no entries, as SDL has none for a file it cannot load.
    NotRead(String),
}

impl List {
    fn entries(&self) -> &[u32] {
        match self {
            List::Read(entries) => entries,
            List::NotSet | List::NotRead(_) => &[],
        }
    }

    /// The variable's value as SDL reads it: `@file` names a file that holds the list, anything else is the list.
    pub(crate) fn read(value: Option<&OsStr>) -> List {
        let Some(value) = value else {
            return List::NotSet;
        };
        let bytes = value.as_encoded_bytes();
        if let Some(path) = bytes.strip_prefix(b"@") {
            // SAFETY: the bytes are `as_encoded_bytes` of an `OsStr`, split right after the valid UTF-8 "@", which
            // `OsStr::from_encoded_bytes_unchecked` allows.
            let path = Path::new(unsafe { OsStr::from_encoded_bytes_unchecked(path) });
            match read_file(path) {
                Ok(text) => List::Read(parse(&text)),
                Err(reason) => List::NotRead(reason),
            }
        } else if bytes.len() > LIST_LIMIT {
            List::NotRead(format!("{} bytes long, more than the {LIST_LIMIT} read", bytes.len()))
        } else {
            List::Read(parse(bytes))
        }
    }
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let shown = path.display();
    let metadata = std::fs::metadata(path).map_err(|e| format!("its file {shown} could not be read ({e})"))?;
    if !metadata.is_file() {
        return Err(format!("{shown} is not a file"));
    }
    let too_long = || format!("its file {shown} is longer than the {LIST_LIMIT} bytes read");
    if metadata.len() > LIST_LIMIT as u64 {
        return Err(too_long());
    }
    let mut text = Vec::new();
    File::open(path)
        .and_then(|file| file.take(LIST_LIMIT as u64 + 1).read_to_end(&mut text))
        .map_err(|e| format!("its file {shown} could not be read ({e})"))?;
    if text.len() > LIST_LIMIT {
        return Err(too_long());
    }
    Ok(text)
}

/// `SDL_LoadVIDPIDListFromHint`'s reading: SDL reads a C string, so the text ends at its first NUL; each "0x" starts a
/// number read as `strtol(spot, &spot, 0)` reads it and cast to 16 bits; a vendor and the product after it make an
/// entry; a vendor with no "0x" after it ends the list.
pub(crate) fn parse(text: &[u8]) -> Vec<u32> {
    let text = match text.iter().position(|&b| b == 0) {
        Some(nul) => &text[..nul],
        None => text,
    };
    let mut entries = Vec::new();
    let mut at = 0;
    while let Some(vendor_at) = find_0x(text, at) {
        let (vendor, end) = strtol_0x(text, vendor_at);
        let Some(product_at) = find_0x(text, end) else {
            break;
        };
        let (product, end) = strtol_0x(text, product_at);
        entries.push(u32::from(vendor) << 16 | u32::from(product));
        at = end;
    }
    entries
}

/// `SDL_strstr(spot, "0x")`.
fn find_0x(text: &[u8], from: usize) -> Option<usize> {
    let rest = text.get(from..)?;
    rest.windows(2).position(|w| w == b"0x").map(|i| from + i)
}

/// `(Uint16)strtol(text + at, &end, 0)` where the text at `at` starts with "0x": the hexadecimal digits after it, the
/// value held at `LONG_MAX` as strtol holds it on overflow (the C library's `long`: 32 bits on Windows, 64 on Linux),
/// then cut to its low 16 bits as the cast cuts it. Without a digit after "0x", strtol reads the "0" alone.
fn strtol_0x(text: &[u8], at: usize) -> (u16, usize) {
    let max = c_long::MAX as u64;
    let start = at + 2;
    let mut end = start;
    let mut value: u64 = 0;
    while let Some(digit) = text.get(end).and_then(|&b| char::from(b).to_digit(16)) {
        value = value.saturating_mul(16).saturating_add(u64::from(digit)).min(max);
        end += 1;
    }
    if end == start {
        (0, at + 1)
    } else {
        (value as u16, end)
    }
}

/// `SDL_GetStringBoolean`'s reading (src/SDL_hints.c:300-309): unset or empty is the default; "0…" or "false" (any
/// case) is false; anything else is true.
fn string_boolean(value: Option<&OsStr>, default: bool) -> bool {
    match value.map(OsStr::as_encoded_bytes) {
        None | Some([]) => default,
        Some(bytes) => !(bytes[0] == b'0' || bytes.eq_ignore_ascii_case(b"false")),
    }
}

/// Whether the process runs under Wine, which SDL asks on Windows only (`#ifdef SDL_PLATFORM_WIN32`).
#[cfg(target_os = "windows")]
fn is_wine() -> bool {
    gilrs_core::utils::is_wine()
}

#[cfg(not(target_os = "windows"))]
fn is_wine() -> bool {
    false
}

/// Why a gamepad is not offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ignored {
    /// Steam's virtual gamepad, and `SDL_GAMECONTROLLER_ALLOW_STEAM_VIRTUAL_GAMEPAD` is not on.
    SteamVirtualGamepad,
    /// `SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT` names pads, not this one.
    NotExcepted,
    /// `SDL_GAMECONTROLLER_IGNORE_DEVICES` names it.
    Listed,
}

impl Display for Ignored {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Ignored::SteamVirtualGamepad => {
                write!(f, "Steam's virtual gamepad, and {ALLOW_STEAM_VIRTUAL_GAMEPAD} is not on")
            }
            Ignored::NotExcepted => write!(f, "{IGNORE_DEVICES_EXCEPT} does not name it"),
            Ignored::Listed => write!(f, "{IGNORE_DEVICES} names it"),
        }
    }
}

/// The variables as read when gilrs starts (SDL reads the lists once when its gamepads start, and the Steam variable
/// at each check from the environment, which does not change while the process runs).
#[derive(Debug)]
pub(crate) struct Rules {
    allow_steam_virtual_gamepad: Option<bool>,
    wine: bool,
    except: List,
    ignore: List,
}

impl Rules {
    pub(crate) fn from_env() -> Rules {
        Rules::new(|name| std::env::var_os(name), is_wine)
    }

    pub(crate) fn new(var: impl Fn(&str) -> Option<OsString>, is_wine: impl FnOnce() -> bool) -> Rules {
        let allow = var(ALLOW_STEAM_VIRTUAL_GAMEPAD);
        let allow_steam_virtual_gamepad = allow.as_deref().map(|v| string_boolean(Some(v), false));
        // SDL asks `WIN_IsWine` only when the Steam virtual gamepad is allowed (SDL_gamepad.c:3294).
        let wine = allow_steam_virtual_gamepad == Some(true) && is_wine();
        Rules {
            allow_steam_virtual_gamepad,
            wine,
            except: List::read(var(IGNORE_DEVICES_EXCEPT).as_deref()),
            ignore: List::read(var(IGNORE_DEVICES).as_deref()),
        }
    }

    /// Whether SDL would not open a gamepad with this vendor, product and version, and why.
    pub(crate) fn check(&self, vendor: u16, product: u16, version: u16) -> Option<Ignored> {
        let allow = self.allow_steam_virtual_gamepad == Some(true);
        if allow && self.wine {
            // SDL: "We are launched by Steam and running under Proton or Wine. We can't tell whether this controller
            // is a Steam Virtual Gamepad, so assume that is doing the appropriate filtering of controllers and
            // anything we see here is fine to use."
            return None;
        }
        if is_steam_virtual_gamepad(vendor, product, version) {
            return (!allow).then_some(Ignored::SteamVirtualGamepad);
        }
        let device = u32::from(vendor) << 16 | u32::from(product);
        let except = self.except.entries();
        if !except.is_empty() {
            (!except.contains(&device)).then_some(Ignored::NotExcepted)
        } else {
            self.ignore.entries().contains(&device).then_some(Ignored::Listed)
        }
    }

    /// One log line: what each variable held.
    pub(crate) fn describe(&self) -> String {
        let mut s = String::from("SDL's gamepad variables: ");
        for (name, list) in [(IGNORE_DEVICES, &self.ignore), (IGNORE_DEVICES_EXCEPT, &self.except)] {
            let _ = write!(s, "{name} ");
            match list {
                List::NotSet => s.push_str("not set"),
                List::NotRead(reason) => {
                    let _ = write!(s, "not read ({reason})");
                }
                List::Read(entries) => describe_entries(&mut s, entries),
            }
            s.push_str("; ");
        }
        let _ = write!(
            s,
            "{ALLOW_STEAM_VIRTUAL_GAMEPAD} {}",
            match self.allow_steam_virtual_gamepad {
                None => "not set",
                Some(true) => "on",
                Some(false) => "off",
            }
        );
        if self.wine {
            s.push_str("; running under Wine: every gamepad is read");
        }
        s
    }
}

/// At most eight entries are written out.
fn describe_entries(s: &mut String, entries: &[u32]) {
    const SHOWN: usize = 8;
    let _ = write!(s, "{} entries", entries.len());
    if entries.is_empty() {
        return;
    }
    s.push_str(" (");
    for (i, entry) in entries.iter().take(SHOWN).enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        let _ = write!(s, "0x{:04x}/0x{:04x}", entry >> 16, entry & 0xffff);
    }
    if entries.len() > SHOWN {
        let _ = write!(s, ", and {} more", entries.len() - SHOWN);
    }
    s.push(')');
}

/// The version in a GUID of SDL's standard layout (bytes 12-13, little-endian; `SDL_GetJoystickGUIDInfo`,
/// SDL_joystick.c:2860-2921), else 0. Only SDL's macOS rule for Steam's virtual pad reads it.
pub(crate) fn guid_version(guid: &[u8; 16]) -> u16 {
    let u16_at = |at: usize| u16::from_le_bytes([guid[at], guid[at + 1]]);
    let bus = u16_at(0);
    let standard = (bus < u16::from(b' ') || bus == 0xff) && u16_at(6) == 0 && u16_at(10) == 0;
    if standard {
        u16_at(12)
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// Steam's virtual gamepad as SDL tells it on this platform.
    #[cfg(not(target_os = "macos"))]
    const STEAM: (u16, u16, u16) = (0x28de, 0x11ff, 1);
    #[cfg(target_os = "macos")]
    const STEAM: (u16, u16, u16) = (0x045e, 0x028e, 0);
    const DUALSENSE: (u16, u16, u16) = (0x054c, 0x0ce6, 0x8111);
    const XBOX_ONE: (u16, u16, u16) = (0x045e, 0x02ea, 0x0301);

    fn rules(vars: &[(&str, &str)], wine: bool) -> Rules {
        Rules::new(|name| vars.iter().find(|(n, _)| *n == name).map(|(_, v)| OsString::from(v)), || wine)
    }

    fn check(rules: &Rules, (vendor, product, version): (u16, u16, u16)) -> Option<Ignored> {
        rules.check(vendor, product, version)
    }

    #[test]
    fn steams_lists_read_as_sdl_reads_them() {
        assert_eq!(parse(b"0x054c/0x0ce6"), [0x054c_0ce6]);
        assert_eq!(parse(b"0x054c/0x0ce6,0x28DE/0x11FF"), [0x054c_0ce6, 0x28de_11ff]);
        assert_eq!(parse(b" 0x054c / 0x0ce6 ;\n0x054c/0x0df2 "), [0x054c_0ce6, 0x054c_0df2]);
    }

    #[test]
    fn the_corners_of_sdls_reading() {
        // Only a lower-case "0x" starts a number: "0X054c" is skipped, "0x0ce6" becomes a vendor without a product.
        assert_eq!(parse(b"0X054c/0x0ce6"), []);
        assert_eq!(parse(b"0x054c"), []);
        // "0x" without a digit reads as 0.
        assert_eq!(parse(b"0x/0x"), [0]);
        // The cast to Uint16 keeps the low 16 bits.
        assert_eq!(parse(b"0x12345/0x1"), [0x2345_0001]);
        // strtol holds an overflowing number at LONG_MAX, whose low 16 bits are 0xffff.
        let long = format!("0x{}/0x1", "f".repeat(40));
        assert_eq!(parse(long.as_bytes()), [0xffff_0001]);
        // 2^32 overflows a 32-bit long (Windows) and fits a 64-bit one (Linux).
        let vendor = if c_long::MAX as u64 >= 1 << 32 { 0 } else { 0xffff };
        assert_eq!(parse(b"0x100000000/0x1"), [vendor << 16 | 1]);
        // SDL reads a C string: the first NUL ends the list.
        assert_eq!(parse(b"0x054c/0x0ce6\0,0x28de/0x11ff"), [0x054c_0ce6]);
        assert_eq!(parse(b"no list at all"), []);
        assert_eq!(parse(b""), []);
    }

    #[test]
    fn hostile_lists_end_without_a_panic() {
        // Bytes of a fixed pseudo-random sequence, "0x" over and over, digits without end, at the size limit.
        let mut seed: u32 = 0x2545_f491;
        let noise: Vec<u8> = (0..LIST_LIMIT)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            })
            .collect();
        let _ = parse(&noise);
        assert_eq!(parse(&b"0x".repeat(LIST_LIMIT / 2)).len(), LIST_LIMIT / 8);
        let digits = [b"0x".as_slice(), &b"f".repeat(LIST_LIMIT - 2)].concat();
        assert_eq!(parse(&digits), []);
        let entries = b"0x1/0x2,".repeat(LIST_LIMIT / 8);
        assert_eq!(parse(&entries).len(), LIST_LIMIT / 8);
    }

    #[test]
    fn a_value_longer_than_the_limit_is_not_read() {
        let at_limit = "0x054c/0x0ce6,".repeat(LIST_LIMIT / 14) + &" ".repeat(LIST_LIMIT % 14);
        assert_eq!(at_limit.len(), LIST_LIMIT);
        assert!(matches!(List::read(Some(OsStr::new(&at_limit))), List::Read(e) if e.len() == LIST_LIMIT / 14));
        let over = at_limit + " ";
        assert!(matches!(List::read(Some(OsStr::new(&over))), List::NotRead(_)));
        assert_eq!(List::read(None), List::NotSet);
        assert_eq!(List::read(Some(OsStr::new(""))), List::Read(Vec::new()));
    }

    #[test]
    fn a_list_in_a_file() {
        let dir = std::env::temp_dir().join(format!("gilrs-t26-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ignore.txt");
        std::fs::write(&file, "0x054c/0x0ce6,0x054c/0x0df2\n").unwrap();
        let at = |path: &Path| {
            let mut value = OsString::from("@");
            value.push(path);
            List::read(Some(&value))
        };
        assert_eq!(at(&file), List::Read(vec![0x054c_0ce6, 0x054c_0df2]));
        // Not there, not a file, too long: no entries, as SDL has none for a file it cannot load.
        assert!(matches!(at(&dir.join("missing.txt")), List::NotRead(_)));
        assert!(matches!(at(&dir), List::NotRead(reason) if reason.contains("is not a file")));
        std::fs::write(&file, vec![b' '; LIST_LIMIT + 1]).unwrap();
        assert!(matches!(at(&file), List::NotRead(reason) if reason.contains("longer than")));
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn steams_virtual_gamepad_is_read_only_when_steam_allows_it() {
        let none = rules(&[], false);
        assert_eq!(check(&none, STEAM), Some(Ignored::SteamVirtualGamepad));
        assert_eq!(check(&none, DUALSENSE), None);
        for off in ["0", "0x1", "false", "FALSE", ""] {
            let r = rules(&[(ALLOW_STEAM_VIRTUAL_GAMEPAD, off)], false);
            assert_eq!(check(&r, STEAM), Some(Ignored::SteamVirtualGamepad), "{off:?}");
        }
        for on in ["1", "yes", "true", "on"] {
            let r = rules(&[(ALLOW_STEAM_VIRTUAL_GAMEPAD, on)], false);
            assert_eq!(check(&r, STEAM), None, "{on:?}");
        }
    }

    #[test]
    fn steams_environment_reads_its_pad_once() {
        // What Steam sets for a game whose DualSense it reads (the variables' names: SDL_hints.h; the values' form:
        // SDL's own documentation of the hints).
        let steam = rules(&[(ALLOW_STEAM_VIRTUAL_GAMEPAD, "1"), (IGNORE_DEVICES, "0x054c/0x0ce6")], false);
        assert_eq!(check(&steam, STEAM), None);
        assert_eq!(check(&steam, DUALSENSE), Some(Ignored::Listed));
        assert_eq!(check(&steam, XBOX_ONE), None);
    }

    #[test]
    fn the_except_list_names_the_only_pads_read() {
        let r = rules(
            &[
                (ALLOW_STEAM_VIRTUAL_GAMEPAD, "1"),
                (IGNORE_DEVICES_EXCEPT, "0x045e/0x02ea"),
                (IGNORE_DEVICES, "0x045e/0x02ea"),
            ],
            false,
        );
        assert_eq!(check(&r, XBOX_ONE), None);
        assert_eq!(check(&r, DUALSENSE), Some(Ignored::NotExcepted));
        // Steam's pad is decided by its own variable, before the lists.
        assert_eq!(check(&r, STEAM), None);
        // An except list in which nothing reads as an entry is no list: the ignore list decides.
        let r = rules(&[(IGNORE_DEVICES_EXCEPT, "none"), (IGNORE_DEVICES, "0x054c/0x0ce6")], false);
        assert_eq!(check(&r, DUALSENSE), Some(Ignored::Listed));
        assert_eq!(check(&r, XBOX_ONE), None);
    }

    #[test]
    fn under_wine_with_steam_every_pad_is_read() {
        let r = rules(&[(ALLOW_STEAM_VIRTUAL_GAMEPAD, "1"), (IGNORE_DEVICES, "0x054c/0x0ce6")], true);
        assert_eq!(check(&r, DUALSENSE), None);
        assert_eq!(check(&r, STEAM), None);
        // Without Steam's permission SDL does not ask whether it runs under Wine.
        let asked = Cell::new(false);
        let r = Rules::new(
            |name| (name == IGNORE_DEVICES).then(|| OsString::from("0x054c/0x0ce6")),
            || {
                asked.set(true);
                true
            },
        );
        assert!(!asked.get());
        assert_eq!(check(&r, DUALSENSE), Some(Ignored::Listed));
        assert_eq!(check(&r, STEAM), Some(Ignored::SteamVirtualGamepad));
    }

    #[test]
    fn the_log_line_names_what_the_variables_held() {
        let r = rules(&[], false);
        assert_eq!(
            r.describe(),
            "SDL's gamepad variables: SDL_GAMECONTROLLER_IGNORE_DEVICES not set; \
             SDL_GAMECONTROLLER_IGNORE_DEVICES_EXCEPT not set; \
             SDL_GAMECONTROLLER_ALLOW_STEAM_VIRTUAL_GAMEPAD not set"
        );
        let many = (0..10).map(|i| format!("0x054c/0x{i:04x}")).collect::<Vec<_>>().join(",");
        let r = rules(&[(ALLOW_STEAM_VIRTUAL_GAMEPAD, "1"), (IGNORE_DEVICES, many.as_str())], false);
        let line = r.describe();
        assert!(line.contains("SDL_GAMECONTROLLER_IGNORE_DEVICES 10 entries (0x054c/0x0000, 0x054c/0x0001,"));
        assert!(line.contains("0x054c/0x0007, and 2 more)"));
        assert!(line.ends_with("SDL_GAMECONTROLLER_ALLOW_STEAM_VIRTUAL_GAMEPAD on"));
    }

    #[test]
    fn the_version_of_a_guid() {
        let g = |s: &str| *uuid::Uuid::parse_str(s).unwrap().as_bytes();
        assert_eq!(guid_version(&g("030000004c050000e60c000011810000")), 0x8111);
        assert_eq!(guid_version(&g("00000000000000000000000000000000")), 0);
        // Not SDL's standard layout (bytes 6-7 set): no version.
        assert_eq!(guid_version(&g("030000004c050000e60c000011810000").map(|b| b | 1)), 0);
    }
}
