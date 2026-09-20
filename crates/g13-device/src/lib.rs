//! The device: claiming it, reading its reports, and the guided run that establishes which bit is
//! which key.
//!
//! Written from libusb's API documentation and from what this device does when spoken to. Nothing here
//! is derived from another driver's source; see PROVENANCE.md.

// a test may unwrap and may fail loudly: a test that cannot panic on a fixture cannot fail
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::print_stdout,
        clippy::print_stderr
    )
)]
pub mod capture;
pub mod joystick;
pub mod keyboard;
pub mod mouse;

use g13_proto::{
    INPUT_BITS, InputReport, PACKET_BYTES, PRODUCT_G13, REPORT_ID_INPUT, VENDOR_LOGITECH,
};
use rusb::UsbContext;
use std::time::Duration;
use thiserror::Error;

/// The interrupt endpoint reports arrive on: eight bytes of key and stick state, read one packet at a time.
pub const ENDPOINT_INPUT: u8 = 0x81;
/// The interrupt endpoint frames leave on: the whole LCD picture in one transfer. The backlight and the M
/// keys' LEDs are control transfers instead.
pub const ENDPOINT_OUTPUT: u8 = 0x02;
/// The device's only interface, and the one claimed on open. The kernel's HID driver is detached from it
/// while this handle holds it, and attached again when the handle is dropped.
pub const INTERFACE: u8 = 0;

/// Why a G13 could not be opened, read from or written to.
#[derive(Debug, Error)]
pub enum DeviceError {
    /// No device on the bus carries Logitech's vendor id and the G13's product id.
    #[error("no G13 (046d:c21c) found on USB")]
    NotFound,
    /// The USB layer refused: opening the device, claiming its interface, or a transfer.
    #[error("usb: {0}")]
    Usb(#[from] rusb::Error),
    /// The device is there, but this user may not open it. Reading and writing it needs the plugdev group.
    #[error(
        "this program needs read and write access to the device: add yourself to the plugdev group"
    )]
    Access,
    /// An input device could not be opened or read.
    #[error("input: {0}")]
    Io(#[from] std::io::Error),
}

/// An open, claimed G13.
pub struct Device {
    /// The claim on the pad's one interface, handed back to the kernel when this is dropped.
    handle: rusb::DeviceHandle<rusb::Context>,
}

impl Device {
    /// Open the first G13 on the bus and claim its one interface.
    ///
    /// The kernel's HID driver must be detached for us to own the interface, which is what
    /// `set_auto_detach_kernel_driver` does. It is put back when this handle is dropped.
    pub fn open() -> Result<Self, DeviceError> {
        let context = rusb::Context::new()?;
        let device = context
            .devices()?
            .iter()
            .find(|device| {
                device
                    .device_descriptor()
                    .map(|d| d.vendor_id() == VENDOR_LOGITECH && d.product_id() == PRODUCT_G13)
                    .unwrap_or(false)
            })
            .ok_or(DeviceError::NotFound)?;

        let handle = device.open()?;
        let _ = handle.set_auto_detach_kernel_driver(true);
        handle
            .claim_interface(INTERFACE)
            .map_err(|error| match error {
                rusb::Error::Access => DeviceError::Access,
                other => DeviceError::Usb(other),
            })?;

        Ok(Self { handle })
    }

    /// Send a frame to the LCD.
    ///
    /// The whole image goes in one interrupt-OUT transfer: report id, the header, then 960 bytes of image.
    /// A short write is reported rather than treated as success, because the device shows whatever arrived.
    pub fn write_lcd(&self, report: &[u8]) -> Result<usize, DeviceError> {
        let written = self.handle.write_interrupt(
            ENDPOINT_OUTPUT,
            report,
            std::time::Duration::from_millis(1000),
        )?;
        Ok(written)
    }

    /// Set the screen's backlight colour.
    ///
    /// A control transfer rather than an interrupt one: the picture goes out on the interrupt endpoint, and the
    /// colour is a feature report. Nothing comes back, so a refusal is returned rather than swallowed - a screen
    /// that ignores a colour looks exactly like a driver that never sent one.
    pub fn write_backlight(&self, red: u8, green: u8, blue: u8) -> Result<(), DeviceError> {
        let report = g13_proto::backlight_report(red, green, blue);
        self.handle.write_control(
            g13_proto::BACKLIGHT_REQUEST_TYPE,
            g13_proto::BACKLIGHT_REQUEST,
            g13_proto::BACKLIGHT_VALUE,
            0,
            &report,
            Duration::from_millis(1000),
        )?;
        Ok(())
    }

    /// Light the four macro keys M1, M2, M3 and MR, or put them out.
    ///
    /// The same kind of transfer as the backlight: a feature report, nothing comes back, and the device keeps no
    /// state across a power cycle - so whatever should be lit has to be said again after every start.
    pub fn write_m_keys(&self, mask: u8) -> Result<(), DeviceError> {
        let report = g13_proto::m_keys_report(mask);
        self.handle.write_control(
            g13_proto::BACKLIGHT_REQUEST_TYPE,
            g13_proto::BACKLIGHT_REQUEST,
            g13_proto::M_KEYS_VALUE,
            0,
            &report,
            Duration::from_millis(1000),
        )?;
        Ok(())
    }

    /// Read one packet from the interrupt endpoint, waiting at most `timeout` for it.
    ///
    /// `Ok(None)` means nothing arrived in time, which is normal: this device reports when something
    /// changes rather than streaming continuously.
    pub fn read_packet(
        &self,
        timeout: Duration,
    ) -> Result<Option<[u8; PACKET_BYTES]>, DeviceError> {
        let mut buffer = [0u8; PACKET_BYTES];
        match self
            .handle
            .read_interrupt(ENDPOINT_INPUT, &mut buffer, timeout)
        {
            // Only the key report is this length and this id; anything else belongs to a report we
            // do not speak yet and must not be interpreted as keys.
            Ok(length) if length == PACKET_BYTES && buffer[0] == REPORT_ID_INPUT => {
                Ok(Some(buffer))
            }
            Ok(_) => Ok(None),
            Err(rusb::Error::Timeout) => Ok(None),
            Err(error) => Err(DeviceError::Usb(error)),
        }
    }

    /// Read reports for as long as `keep_going` says so, handing each one to `on_report`.
    ///
    /// The callback sees the report plus the bits that changed, which is what makes a guided mapping
    /// run possible: a key going down is a bit appearing that was not there before.
    pub fn watch<F>(&self, mut on_report: F) -> Result<(), DeviceError>
    where
        F: FnMut(&InputReport, &[usize], &[usize]) -> bool,
    {
        let mut previous = InputReport::default();
        loop {
            if let Some(packet) = self.read_packet(Duration::from_millis(500))?
                && let Some(report) = InputReport::from_packet(&packet)
            {
                let (pressed, released) = report.changes_from(&previous);
                previous = report;
                if !on_report(&report, &pressed, &released) {
                    return Ok(());
                }
            }
        }
    }
}

/// The controls this pad has, read off the device's own legends.
///
/// This is not the predecessor's vocabulary and is not derived from its configuration: it is what is
/// printed on the hardware, transcribed from it. `G1`..`G22` are the key
/// columns and the run beneath them; `LR` is the round button beside the L keys; `LMB` and `RMB` are the
/// two buttons in the thumb area, which report through the same physical switch as the left and right
/// mouse buttons; and `JCLICK` is pressing the stick down.
///
/// **A control is something that sets a bit.** The words `JUP`, `JDOWN`, `JLEFT` and `JRIGHT` are printed
/// around the stick, but they are not controls: they do not set a bit, they describe where a two-axis stick
/// has been pushed. Treating them as controls put them in two places at once - listed beside the buttons here
/// and counted as directions by the stick - so they are deliberately absent and live with the stick, in
/// `g13-config`'s `stick` module, where the turn is broken into sectors that each carry their own binding.
/// The stick is measured by its calibration, not by walking the pad asking for one bit per direction.
///
/// A first transcription also listed `LB1`. It is the same physical button as `LMB`  -  one switch, two
/// readings  -  so it is not a control and is deliberately absent here rather than mapped twice.
pub const CONTROLS: [&str; 34] = [
    "LR", "L1", "L2", "L3", "L4", "M1", "M2", "M3", "MR", "G1", "G2", "G3", "G4", "G5", "G6", "G7",
    "G8", "G9", "G10", "G11", "G12", "G13", "G14", "G15", "G16", "G17", "G18", "G19", "G20", "G21",
    "G22", "LMB", "RMB", "JCLICK",
];

/// Controls whose reports are continuous rather than a single press, so they are read as an aggregate
/// of the states they pass through rather than as one bit.
///
/// Only the stick's own click: the stick's *positions* are not controls at all (see `CONTROLS`), and it is
/// read as two axes by the calibration rather than bit by bit.
pub const CONTINUOUS: [&str; 1] = ["JCLICK"];

/// Every input device the system has, as `(name, event path)`.
///
/// Read from `/proc/bus/input/devices` rather than by opening each device: opening needs permission, and a
/// list that silently omits what it cannot open hides exactly the device you are looking for. The kernel's
/// own list needs nothing.
pub fn input_devices() -> Vec<(String, std::path::PathBuf)> {
    match std::fs::read_to_string("/proc/bus/input/devices") {
        Ok(text) => parse_proc_devices(&text),
        Err(_) => Vec::new(),
    }
}

/// Parse the kernel's device list: blocks separated by blank lines, a `N: Name="..."` line and an
/// `H: Handlers=... eventN ...` line.
pub fn parse_proc_devices(text: &str) -> Vec<(String, std::path::PathBuf)> {
    let mut devices = Vec::new();
    let mut name: Option<String> = None;
    for block in text.split("\n\n") {
        let mut this_name = None;
        let mut event = None;
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("N: Name=") {
                this_name = Some(rest.trim().trim_matches('"').to_string());
            }
            if let Some(rest) = line.strip_prefix("H: Handlers=") {
                event = rest
                    .split_whitespace()
                    .find(|handler| handler.starts_with("event"))
                    .map(|handler| std::path::PathBuf::from("/dev/input/").join(handler));
            }
        }
        if let (Some(this_name), Some(event)) = (this_name, event) {
            devices.push((this_name, event));
        }
    }
    let _ = name.take();
    devices
}

/// Open an input device by path, to read what something else is emitting.
pub fn open_input(path: &std::path::Path) -> Result<evdev::Device, DeviceError> {
    evdev::Device::open(path).map_err(DeviceError::Io)
}

/// Wait for the next key event from a device, as `(keycode, pressed)`.
///
/// Blocks until the device produces one, which is why callers run it on its own thread. Used to observe
/// what another driver sends: behaviour, not source.
/// One press from a real device, as the kernel reported it.
///
/// A press is not always a key. A keyboard key, a mouse button and a gamepad's face buttons are key events,
/// but **a gamepad's d-pad and its two triggers are not** - a real pad reports them as absolute axes, which is
/// what the X-Box 360 pad on this machine advertises: `ABS_HAT0X`/`ABS_HAT0Y` for the d-pad and `ABS_Z`/
/// `ABS_RZ` for the triggers, and no `BTN_DPAD_*` at all. A reader that only looks at key events therefore
/// never sees a d-pad or a trigger being pressed, and a binding can never be set by pressing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
    /// A key going down: a keyboard key, a mouse button or a gamepad button.
    Key(u16),
    /// An absolute axis arriving at a position that means a press.
    Axis {
        /// The kernel's `ABS_*` code for the axis the pad moved.
        code: u16,
        /// Where it arrived: a hat is -1 or 1, and a trigger anything above resting.
        value: i32,
    },
}

/// Whether an absolute axis at this value means something was pressed.
///
/// A hat rests at 0 and is -1 or 1 when pushed; a trigger rests at nothing and only travels one way. An axis
/// arriving anywhere else is a stick moving, which is not a press and has nothing to bind.
pub fn axis_means_a_press(code: u16, value: i32) -> bool {
    use evdev::AbsoluteAxisCode as Abs;
    match Abs(code) {
        Abs::ABS_HAT0X | Abs::ABS_HAT0Y => value != 0,
        Abs::ABS_Z | Abs::ABS_RZ => value > 0,
        _ => false,
    }
}

/// The next press from a device: a key going down, or an absolute axis reaching a position that means one.
///
/// Both, because a real pad reports its d-pad and its triggers on axes rather than as keys. Nothing else counts
/// - a release is the other half of a press already reported, and a stick moving is not a button.
pub fn next_press(device: &mut evdev::Device) -> Result<Press, DeviceError> {
    loop {
        let mut fetch = device.fetch_events()?;
        for event in fetch.by_ref() {
            match event.event_type() {
                evdev::EventType::KEY => {
                    if event.value() == 1 {
                        return Ok(Press::Key(event.code()));
                    }
                }
                evdev::EventType::ABSOLUTE if axis_means_a_press(event.code(), event.value()) => {
                    return Ok(Press::Axis {
                        code: event.code(),
                        value: event.value(),
                    });
                }
                _ => {}
            }
        }
    }
}

/// The next key event from a device, as `(keycode, pressed)`.
///
/// Key events only, unlike `next_press`: a macro recorder needs both halves of every press, and an absolute
/// axis is not a key the macro format can hold.
pub fn next_key(device: &mut evdev::Device) -> Result<(u16, bool), DeviceError> {
    loop {
        let mut fetch = device.fetch_events()?;
        for event in fetch.by_ref() {
            if event.event_type() == evdev::EventType::KEY {
                return Ok((event.code(), event.value() == 1));
            }
        }
    }
}

/// Bit indices this run has not heard about, out of the 56 the device can report.
pub fn unaccounted(seen: &[usize]) -> Vec<usize> {
    (0..INPUT_BITS).filter(|bit| !seen.contains(bit)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_d_pad_or_a_trigger_is_a_press_and_a_stick_moving_is_not() {
        use evdev::AbsoluteAxisCode as Abs;
        // a hat is pushed in one of four directions, and rests in the middle
        assert!(axis_means_a_press(Abs::ABS_HAT0Y.0, -1), "d-pad up");
        assert!(axis_means_a_press(Abs::ABS_HAT0Y.0, 1), "d-pad down");
        assert!(axis_means_a_press(Abs::ABS_HAT0X.0, 1), "d-pad right");
        assert!(!axis_means_a_press(Abs::ABS_HAT0Y.0, 0), "a hat at rest");
        // a trigger only travels one way, so any travel is a press
        assert!(axis_means_a_press(Abs::ABS_Z.0, 255), "the left trigger");
        assert!(
            axis_means_a_press(Abs::ABS_RZ.0, 1),
            "the right trigger, barely"
        );
        assert!(!axis_means_a_press(Abs::ABS_Z.0, 0), "a trigger at rest");
        // and a stick is not a press: pressing one to set a binding would bind the wrong thing entirely
        assert!(!axis_means_a_press(Abs::ABS_X.0, -32767));
        assert!(!axis_means_a_press(Abs::ABS_RY.0, 12000));
    }

    #[test]
    fn unaccounted_lists_the_bits_nobody_has_explained() {
        let seen = vec![0usize, 5, 9];
        let missing = unaccounted(&seen);
        assert_eq!(missing.len(), INPUT_BITS - 3);
        assert!(!missing.contains(&0));
        assert!(missing.contains(&1));
    }

    #[test]
    fn the_report_id_we_expect_is_the_one_the_descriptor_declared() {
        assert_eq!(REPORT_ID_INPUT, 0x01);
        assert_eq!(PACKET_BYTES, 8);
    }
}

#[cfg(test)]
mod control_list_tests {
    use super::*;

    #[test]
    fn the_list_is_the_one_read_off_the_device() {
        assert_eq!(CONTROLS.len(), 34);
        assert_eq!(CONTROLS[0], "LR");
        assert!(CONTROLS.contains(&"LMB") && CONTROLS.contains(&"RMB"));
        assert!(
            CONTROLS.contains(&"JCLICK"),
            "pressing the stick down is a control"
        );
        // and the words around the stick are not, because they set no bit
        for direction in ["JUP", "JDOWN", "JLEFT", "JRIGHT"] {
            assert!(
                !CONTROLS.contains(&direction),
                "{direction} sets no bit, so it is a sector of the stick rather than a control"
            );
        }
    }

    #[test]
    fn lb1_is_not_a_control_because_it_is_the_same_button_as_lmb() {
        assert!(!CONTROLS.contains(&"LB1"));
        assert!(CONTROLS.contains(&"LMB"));
    }

    #[test]
    fn there_are_twenty_two_g_keys_and_no_duplicates() {
        let g: Vec<&&str> = CONTROLS
            .iter()
            .filter(|name| name.starts_with('G'))
            .collect();
        assert_eq!(g.len(), 22);
        let mut sorted = CONTROLS.to_vec();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len(), "no control appears twice");
    }

    #[test]
    fn the_continuous_ones_are_controls_too() {
        for name in CONTINUOUS {
            assert!(
                CONTROLS.contains(&name),
                "{name} must be one of the controls"
            );
        }
    }
}

#[cfg(test)]
mod proc_device_tests {
    use super::*;

    const SAMPLE: &str = "I: Bus=0003 Vendor=046d Product=c21c Version=0111\n\
N: Name=\"G13\"\n\
P: Phys=usb-0000:00:14.0-6/input1\n\
H: Handlers=sysrq rfkill kbd event27 js5\n\
B: PROP=0\n\
\n\
I: Bus=0003 Vendor=1234 Product=5678 Version=0001\n\
N: Name=\"Something Else\"\n\
H: Handlers=mouse0 event9\n\
B: PROP=0\n";

    #[test]
    fn names_and_event_nodes_are_paired() {
        let devices = parse_proc_devices(SAMPLE);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].0, "G13");
        assert_eq!(devices[0].1.to_string_lossy(), "/dev/input/event27");
        assert_eq!(devices[1].0, "Something Else");
        assert_eq!(devices[1].1.to_string_lossy(), "/dev/input/event9");
    }

    #[test]
    fn a_block_with_no_event_node_is_skipped() {
        let devices = parse_proc_devices("N: Name=\"Nameless\"\nH: Handlers=kbd\n");
        assert!(devices.is_empty());
    }

    #[test]
    fn an_empty_file_is_not_a_panic() {
        assert!(parse_proc_devices("").is_empty());
    }
}
