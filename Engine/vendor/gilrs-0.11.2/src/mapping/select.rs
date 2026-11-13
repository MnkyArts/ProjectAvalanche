// Project Avalanche patches T22 / T23 (Engine/vendor/PATCHES.md): which mapping a gamepad gets on Linux, in SDL's
// order (SDL 3.4.20, `SDL_PrivateGetGamepadMapping`, src/joystick/SDL_gamepad.c:2368-2392): a row of the mapping list
// (`MappingDb::lookup`), else SDL's automatic mapping (`automatic`), else SDL's default mapping, which gilrs does not
// have: there the device keeps gilrs' default mapping, as before these patches.

use super::automatic::{self, Capabilities};
use super::{Mapping, MappingDb, Origin};

use uuid::Uuid;

/// SDL's reading of a joystick GUID (SDL 3.4.20 `src/joystick/SDL_joystick.c`). gilrs' Linux GUIDs have SDL's
/// standard layout (`SDL_CreateJoystickGUID` for a device with a vendor, 2933-2975): bus, name CRC (gilrs: 0), vendor,
/// 0, product, 0, version, driver signature and data (gilrs: 0), each 16-bit little-endian.
pub(crate) mod sdl_guid {
    /// `SDL_HARDWARE_BUS_VIRTUAL` (src/joystick/SDL_sysjoystick.h:155).
    const BUS_VIRTUAL: u16 = 0xff;

    fn u16_at(guid: &[u8; 16], at: usize) -> u16 {
        u16::from_le_bytes([guid[at], guid[at + 1]])
    }

    /// `SDL_GetJoystickGUIDInfo` (2860-2921): vendor and product of a GUID in the standard form, else none.
    fn vendor_product(guid: &[u8; 16]) -> Option<(u16, u16)> {
        let bus = u16_at(guid, 0);
        let standard = (bus < u16::from(b' ') || bus == BUS_VIRTUAL)
            && u16_at(guid, 6) == 0
            && u16_at(guid, 10) == 0;
        standard.then(|| (u16_at(guid, 4), u16_at(guid, 8)))
    }

    /// `SDL_GetJoystickVendor` of the device: the GUID's vendor, 0 when it has none. SDL builds a device without a
    /// vendor a name-based GUID instead (`SDL_CreateJoystickGUID`); gilrs' GUID of it reads vendor 0 here, as SDL's
    /// reads unless the name is at most two bytes long.
    pub(crate) fn vendor(guid: &[u8; 16]) -> u16 {
        vendor_product(guid).map_or(0, |(vendor, _)| vendor)
    }

    /// `SDL_JoystickGUIDUsesVersion` (3132-3146): not an MFi GUID (`SDL_IsJoystickMFI`: byte 14 is `'m'`), and the
    /// GUID names a vendor and a product.
    pub(crate) fn uses_version(guid: &[u8; 16]) -> bool {
        guid[14] != b'm'
            && vendor_product(guid).is_some_and(|(vendor, product)| vendor != 0 && product != 0)
    }

    /// The GUID with its version cleared (`SDL_SetJoystickGUIDVersion(guid, 0)`, 2996-3001).
    pub(crate) fn without_version(guid: &[u8; 16]) -> [u8; 16] {
        let mut guid = *guid;
        guid[12] = 0;
        guid[13] = 0;
        guid
    }
}

/// The mapping of a gamepad that has just connected, and one log line that says where it came from: the device's
/// name, GUID, vendor and product, and the row (with its origin), SDL's automatic mapping (with the elements SDL would
/// write for it), or gilrs' default mapping.
pub(crate) fn select(db: &MappingDb, gamepad: &gilrs_core::Gamepad) -> Mapping {
    let uuid = Uuid::from_bytes(gamepad.uuid());
    let id = |v: Option<u16>| v.map_or_else(|| "none".to_owned(), |v| format!("{v:04x}"));
    let device = format!(
        "Gamepad \"{}\" (GUID {}, vendor {}, product {})",
        gamepad.name(),
        uuid.simple(),
        id(gamepad.vendor_id()),
        id(gamepad.product_id()),
    );

    if let Some(found) = db.lookup(uuid) {
        let origin = match found.origin {
            Origin::Database => "the bundled SDL database",
            Origin::Application => "the application's mappings",
            Origin::Environment => "SDL_GAMECONTROLLERCONFIG",
        };
        let matched = if found.without_version {
            format!(", row {} matched without the version", found.guid.simple())
        } else {
            String::new()
        };
        return match Mapping::parse_sdl_mapping(found.row, gamepad.buttons(), gamepad.axes()) {
            Ok(mapping) => {
                info!(
                    "{device}: mapping \"{}\" from {origin}{matched}",
                    mapping.name()
                );
                mapping
            }
            Err(e) => {
                warn!("{device}: the row from {origin}{matched} could not be read ({e}); gilrs' default mapping");
                Mapping::default(gamepad)
            }
        };
    }

    match automatic::generate(&Capabilities::of(gamepad)) {
        Some(generated) => {
            info!(
                "{device}: no mapping row; SDL's automatic mapping {}",
                generated.sdl_elements()
            );
            generated.mapping(gamepad.name())
        }
        None => {
            // PORT(choice): a device without BTN_GAMEPAD that no row names keeps gilrs' default mapping (evdev
            // codes to elements, ABS_Z/ABS_RZ as LeftZ/RightZ) and stays a gamepad, as before this patch; SDL 3.4.20
            // on Linux has no default mapping (`s_pDefaultMapping`, SDL_gamepad.c:2389-2391, is set only by a
            // "default" row, which SDL_gamepad_db.h has for Emscripten only) and would not open it as a gamepad; the
            // console read XInput pads only, which need no mapping.
            info!("{device}: no mapping row, and SDL makes no automatic mapping (no BTN_GAMEPAD); gilrs' default mapping");
            Mapping::default(gamepad)
        }
    }
}
