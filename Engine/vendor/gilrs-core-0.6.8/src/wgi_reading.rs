// Project Avalanche patch T25 (Engine/vendor/PATCHES.md): how the Windows.Gaming.Input backend reads a controller: a
// raw controller's GUID and axis values (moved here unchanged from `platform/windows_wgi/gamepad.rs`), and which of
// its polled readings the backend takes and what it compares a taken reading with (changed). Kept apart from the
// backend, which builds on Windows only, so that the tests run on every platform.
//
// SDL 3.4.20 takes a reading when its timestamp is 0 or differs from the last taken reading's
// (`WGI_JoystickUpdate`, src/joystick/windows/SDL_windows_gaming_input.c:856) and sends each value against its own
// state, that is against the last taken reading. gilrs-core 0.6.8 swapped its two readings before it looked at the
// timestamp: a reading it skipped became the base of the next comparison, so a change that reading carried (a press, a
// release) was never reported, and a controller whose timestamp stays 0 reported nothing at all; a failed read was
// compared all the same, against the buffer of two polls before.

use uuid::Uuid;

/// A raw controller's GUID (one that WGI does not read as a `Gamepad`), as gilrs-core 0.6.8 builds it: SDL's layout
/// with the bus always USB (SDL's WGI backend writes Bluetooth for a wireless controller; gilrs-core keeps USB, under
/// which the database's Windows rows are written), the vendor and the product, version 0. The DualSense
/// (054c:0ce6): `030000004c050000e60c000000000000`.
pub(crate) fn raw_controller_guid(vendor: u16, product: u16) -> Uuid {
    const SDL_HARDWARE_BUS_USB: u16 = 0x03;
    let mut guid = [0; 16];
    guid[0..2].copy_from_slice(&SDL_HARDWARE_BUS_USB.to_le_bytes());
    guid[4..6].copy_from_slice(&vendor.to_le_bytes());
    guid[8..10].copy_from_slice(&product.to_le_bytes());
    Uuid::from_bytes(guid)
}

/// A raw controller's axis (WGI reads it 0…1) as gilrs-core reports it, −32768…32767 (its range in `axis_info`). SDL's
/// conversion (`(Sint16)((int)(axes[i] * 65535) - 32768)`, src/joystick/windows/SDL_windows_gaming_input.c:884) in
/// gilrs-core 0.6.8's order: the product is cut after the subtraction, SDL cuts it before (at most one step apart).
pub(crate) fn raw_axis_value(value: f64) -> i32 {
    ((value * 65535.0) - 32768.0) as i32
}

/// A polled reading's timestamp, as WGI returns it (`RawGameController::GetCurrentReading`, `GamepadReading::Timestamp`).
pub(crate) trait Timestamped {
    fn timestamp(&self) -> u64;
}

/// The last reading taken from a controller and the buffer the next poll reads into.
#[derive(Debug)]
pub(crate) struct Readings<R> {
    taken: R,
    polled: R,
}

impl<R: Clone + Timestamped> Readings<R> {
    /// The controller's first reading: the base of the first comparison, reported as nothing (as gilrs-core 0.6.8).
    pub(crate) fn new(first: R) -> Self {
        Readings { polled: first.clone(), taken: first }
    }

    /// Polls once: `read` writes the controller's current reading into the buffer. A reading SDL takes (timestamp 0,
    /// or not the last taken one's) is handed to `report` after the last taken reading and becomes the new base. A
    /// skipped reading or a failed read changes nothing. Answers whether the reading was taken.
    pub(crate) fn poll<E>(
        &mut self,
        read: impl FnOnce(&mut R) -> Result<(), E>,
        report: impl FnOnce(&R, &R),
    ) -> Result<bool, E> {
        read(&mut self.polled)?;
        let timestamp = self.polled.timestamp();
        if timestamp != 0 && timestamp == self.taken.timestamp() {
            return Ok(false);
        }
        report(&self.taken, &self.polled);
        std::mem::swap(&mut self.taken, &mut self.polled);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::{raw_axis_value, raw_controller_guid, Readings, Timestamped};
    use uuid::Uuid;

    #[test]
    fn the_guids_of_sonys_pads_are_the_databases_windows_rows() {
        let guid = |vendor, product| raw_controller_guid(vendor, product).simple().to_string();
        // gamecontrollerdb.txt's Windows rows for the DualSense, the DualSense Edge and the DualShock 4 (two models).
        assert_eq!(guid(0x054c, 0x0ce6), "030000004c050000e60c000000000000");
        assert_eq!(guid(0x054c, 0x0df2), "030000004c050000f20d000000000000");
        assert_eq!(guid(0x054c, 0x05c4), "030000004c050000c405000000000000");
        assert_eq!(guid(0x054c, 0x09cc), "030000004c050000cc09000000000000");
        // gilrs-core 0.6.8's own expression gives the same GUID for every vendor and product tried.
        let published = |vendor: u16, product: u16| {
            let (vendor_id, product_id, version, bustype) = (vendor.to_be(), product.to_be(), 0u16, 0x03u32.to_be());
            Uuid::from_fields(
                bustype,
                vendor_id,
                0,
                &[(product_id >> 8) as u8, product_id as u8, 0, 0, (version >> 8) as u8, version as u8, 0, 0],
            )
        };
        for vendor in [0, 1, 0x054c, 0x045e, 0x28de, 0xff00, 0xffff] {
            for product in [0, 1, 0x0ce6, 0x11ff, 0x8000, 0xffff] {
                assert_eq!(raw_controller_guid(vendor, product), published(vendor, product));
            }
        }
    }

    #[test]
    fn a_raw_axis_spans_the_reported_range() {
        // A trigger at rest reads 0 in WGI: the bottom of the range, which gilrs reads as a released button.
        assert_eq!(raw_axis_value(0.0), -32768);
        assert_eq!(raw_axis_value(1.0), 32767);
        assert_eq!(raw_axis_value(0.5), 0);
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Reading {
        timestamp: u64,
        pressed: bool,
    }

    impl Timestamped for Reading {
        fn timestamp(&self) -> u64 {
            self.timestamp
        }
    }

    const fn r(timestamp: u64, pressed: bool) -> Reading {
        Reading { timestamp, pressed }
    }

    /// Polls `polls` in turn (`None`: the read fails after writing a junk reading into the buffer) and answers the
    /// changes reported, as the backend's comparison would send them: (from, to) of `pressed`.
    fn changes(first: Reading, polls: &[Option<Reading>]) -> Vec<(bool, bool)> {
        let mut readings = Readings::new(first);
        let mut changes = Vec::new();
        for poll in polls {
            let _ = readings.poll(
                |buffer| {
                    *buffer = poll.unwrap_or(r(99, !buffer.pressed));
                    poll.map(|_| ()).ok_or(())
                },
                |old, new| {
                    if old.pressed != new.pressed {
                        changes.push((old.pressed, new.pressed));
                    }
                },
            );
        }
        changes
    }

    #[test]
    fn a_change_in_a_reading_with_an_unchanged_timestamp_is_reported_with_the_next_taken_one() {
        // gilrs-core 0.6.8 made the skipped (1, pressed) reading the base and compared (2, pressed) with it: the press
        // was never reported.
        assert_eq!(changes(r(1, false), &[Some(r(1, true)), Some(r(2, true))]), [(false, true)]);
        // The same for a release: the button would have stayed held.
        assert_eq!(
            changes(r(1, false), &[Some(r(2, true)), Some(r(2, false)), Some(r(3, false))]),
            [(false, true), (true, false)]
        );
    }

    #[test]
    fn a_controller_whose_timestamp_stays_zero_is_read_at_every_poll() {
        assert_eq!(
            changes(r(0, false), &[Some(r(0, true)), Some(r(0, true)), Some(r(0, false))]),
            [(false, true), (true, false)]
        );
    }

    #[test]
    fn a_failed_read_reports_nothing_and_keeps_the_base() {
        // gilrs-core 0.6.8 compared the failed read's buffer (two polls old) and reported a release and a new press.
        assert_eq!(changes(r(1, false), &[Some(r(2, true)), None, Some(r(3, true))]), [(false, true)]);
    }

    #[test]
    fn an_unchanged_reading_reports_no_change() {
        assert_eq!(changes(r(1, true), &[Some(r(1, true)), Some(r(2, true))]), []);
    }
}
