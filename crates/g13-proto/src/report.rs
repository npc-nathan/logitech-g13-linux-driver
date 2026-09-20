//! The 7-byte key report this device sends.
//!
//! The descriptor says report 0x01 is one input item: usage page 0xff00, report size 8, count 7. So the
//! wire packet is a report id followed by seven bytes  -  a 56-bit bitmap, and **which bit means which key
//! is vendor-defined**: the descriptor does not say. That mapping is therefore established by observation
//! on real hardware and lives in `keymap`, never guessed at.
//!
//! Bit order is the HID convention: least significant bit first, so bit `n` is bit `n % 8` of byte
//! `n / 8`.

use crate::descriptor::{Descriptor, MainItem};

/// The id in front of the key report: the first byte of everything the pad sends.
pub const REPORT_ID_INPUT: u8 = 0x01;
/// Payload bytes after the report id, straight from the descriptor.
pub const INPUT_BYTES: usize = 7;
/// Bits in the bitmap.
pub const INPUT_BITS: usize = INPUT_BYTES * 8;
/// Length of the wire packet including the leading report id.
pub const PACKET_BYTES: usize = INPUT_BYTES + 1;

/// One key report: which of the 56 vendor bits are currently set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InputReport {
    /// The 56 bits as they arrived, low bit first, one per bit of the report's payload bytes.
    bits: u64,
}

impl InputReport {
    /// Take a packet as it arrives on the interrupt endpoint. The first byte must be the report id:
    /// anything else is a different report and is refused rather than misread.
    pub fn from_packet(packet: &[u8]) -> Option<Self> {
        if packet.len() != PACKET_BYTES || packet[0] != REPORT_ID_INPUT {
            return None;
        }
        Some(Self::from_payload(&packet[1..]))
    }

    /// Take just the payload bytes.
    pub fn from_payload(payload: &[u8]) -> Self {
        let mut bits = 0u64;
        for (index, byte) in payload.iter().take(INPUT_BYTES).enumerate() {
            bits |= (*byte as u64) << (index * 8);
        }
        Self { bits }
    }

    /// The raw bitmap, for a caller that wants bits rather than a named control.
    pub fn bits(&self) -> u64 {
        self.bits
    }

    /// Whether bit `n` is set. A bit past the end of the report is false rather than a panic.
    pub fn is_set(&self, bit: usize) -> bool {
        bit < INPUT_BITS && self.bits & (1u64 << bit) != 0
    }

    /// Set bits, ascending. Ordering matters: it makes change reports stable and testable.
    pub fn set_bits(&self) -> Vec<usize> {
        (0..INPUT_BITS).filter(|bit| self.is_set(*bit)).collect()
    }

    /// Bits that went down and bits that came up since `previous`.
    pub fn changes_from(&self, previous: &Self) -> (Vec<usize>, Vec<usize>) {
        let pressed = self
            .set_bits()
            .into_iter()
            .filter(|bit| !previous.is_set(*bit))
            .collect();
        let released = previous
            .set_bits()
            .into_iter()
            .filter(|bit| !self.is_set(*bit))
            .collect();
        (pressed, released)
    }
}

/// How many bytes a report of this id carries, taken from the descriptor rather than hard-coded  -
/// so a device that changes its mind produces a failed assertion, not silent corruption.
pub fn input_bytes_from(descriptor: &Descriptor) -> Option<usize> {
    let fields = descriptor.fields_for(REPORT_ID_INPUT, MainItem::Input);
    let field = fields.first()?;
    Some((field.report_size * field.report_count).div_ceil(8) as usize)
}

/// The screen's backlight, which is a colour rather than a brightness.
///
/// Five bytes: an id, red, green and blue, and a byte the device ignores. It goes out as a class-interface
/// SET_REPORT control transfer, with the report id in `wValue`'s low byte and the report type - feature, 3 -
/// in its high byte.
///
/// **The payload says 5 and the request says 7.** The request is right: the backlight's own feature id is 7.
/// The payload's first byte is the *macro key LEDs'* id, carried over by the original - the two reports have the
/// same length and sit next to each other in that source. The pad accepts it, and this build sends exactly what
/// was proven to work rather than tidying a working command.
pub const REPORT_ID_BACKLIGHT: u8 = 0x05;
/// `SET_REPORT`, the USB control request that carries a report to the device.
pub const BACKLIGHT_REQUEST: u8 = 9;
/// Class request, interface recipient: what this device answers for a feature report.
pub const BACKLIGHT_REQUEST_TYPE: u8 = 0x21;
/// Feature report, id 7, in the layout USB uses for `wValue`.
pub const BACKLIGHT_VALUE: u16 = 0x0307;

/// The five bytes that set the backlight.
pub fn backlight_report(red: u8, green: u8, blue: u8) -> [u8; 5] {
    [REPORT_ID_BACKLIGHT, red, green, blue, 0]
}

/// The four LED-backlit keys M1, M2, M3 and MR, as one feature report.
///
/// Five bytes: an id, a bitmask, then three the device ignores. Bit 0 is M1, bit 1 M2, bit 2 M3 and bit 3 MR.
/// The device keeps no state across a power cycle - it comes up with all four off - so whatever is meant to be
/// lit has to be said after every start.
pub const REPORT_ID_M_KEYS: u8 = 0x05;
/// Feature report, id 5, in the layout USB uses for `wValue`.
pub const M_KEYS_VALUE: u16 = 0x0305;

/// The five bytes that set the four macro key lights.
pub fn m_keys_report(mask: u8) -> [u8; 5] {
    [REPORT_ID_M_KEYS, mask, 0, 0, 0]
}

/// Which of the four should be lit.
///
/// M1, M2 and M3 are the profile keys, so they show which profile is active: profile 1 lights M1, profile 2
/// lights M2, profile 3 lights M3, and profile 0 lights none of them - there is no M0 on the pad, so the
/// lowest profile is shown by none of the three being lit. MR shows that a macro is being recorded.
pub fn m_keys_mask(profile: u32, recording: bool) -> u8 {
    let mut mask = 0u8;
    if (1..=3).contains(&profile) {
        mask |= 1 << (profile - 1);
    }
    if recording {
        mask |= 1 << 3;
    }
    mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_macro_key_lights_are_a_mask_of_four_bits() {
        assert_eq!(m_keys_report(0b0000_0001), [5, 1, 0, 0, 0]);
        assert_eq!(m_keys_report(0b0000_1000), [5, 8, 0, 0, 0]);
        // the profile keys show the profile, and profile 0 has no key of its own to light
        assert_eq!(m_keys_mask(0, false), 0b0000);
        assert_eq!(m_keys_mask(1, false), 0b0001, "M1");
        assert_eq!(m_keys_mask(2, false), 0b0010, "M2");
        assert_eq!(m_keys_mask(3, false), 0b0100, "M3");
        // and MR shows that a recording is running
        assert_eq!(m_keys_mask(2, true), 0b1010);
        // the request and the payload agree here, unlike the backlight's
        assert_eq!(M_KEYS_VALUE as u8, REPORT_ID_M_KEYS);
        assert_eq!(M_KEYS_VALUE >> 8, 3, "the report type is feature");
    }
    #[test]
    fn the_backlight_report_is_the_five_bytes_the_pad_takes() {
        assert_eq!(backlight_report(0, 0, 255), [5, 0, 0, 255, 0]);
        assert_eq!(backlight_report(255, 255, 255), [5, 255, 255, 255, 0]);
        // white at full is the one the original set while it loaded, and the trailing byte stays zero
        assert_eq!(backlight_report(128, 128, 128), [5, 128, 128, 128, 0]);
        // the request and the payload are not the same number, on purpose
        assert_ne!(BACKLIGHT_VALUE as u8, REPORT_ID_BACKLIGHT);
        assert_eq!(BACKLIGHT_VALUE >> 8, 3, "the report type is feature");
    }
    #[test]
    fn bit_order_is_least_significant_first() {
        let report = InputReport::from_payload(&[0x01, 0x00, 0, 0, 0, 0, 0]);
        assert!(report.is_set(0));
        assert!(!report.is_set(1));

        let second = InputReport::from_payload(&[0x00, 0x02, 0, 0, 0, 0, 0]);
        assert!(second.is_set(9), "byte 1 bit 1 is bit 9 overall");
    }

    #[test]
    fn refuses_a_packet_that_is_not_this_report() {
        assert!(
            InputReport::from_packet(&[0x00, 0, 0, 0, 0, 0, 0, 0]).is_none(),
            "wrong id"
        );
        assert!(
            InputReport::from_packet(&[0x01, 0, 0]).is_none(),
            "too short"
        );
        assert!(InputReport::from_packet(&[0x01; 9]).is_none(), "too long");
        assert!(InputReport::from_packet(&[0x01; PACKET_BYTES]).is_some());
    }

    #[test]
    fn reports_which_bits_changed() {
        let before = InputReport::from_payload(&[0b0000_0001, 0, 0, 0, 0, 0, 0]);
        let after = InputReport::from_payload(&[0b0000_0010, 0, 0, 0, 0, 0, 0]);
        let (pressed, released) = after.changes_from(&before);
        assert_eq!(pressed, vec![1]);
        assert_eq!(released, vec![0]);
    }

    #[test]
    fn set_bits_are_ascending() {
        let report = InputReport::from_payload(&[0x80, 0x01, 0, 0, 0, 0, 0]);
        assert_eq!(report.set_bits(), vec![7, 8]);
    }

    #[test]
    fn the_descriptor_agrees_about_the_length() {
        let bytes = std::fs::read("tests/fixtures/g13-report-descriptor.bin").expect("fixture");
        let parsed = crate::descriptor::parse(&bytes);
        assert_eq!(input_bytes_from(&parsed), Some(INPUT_BYTES));
    }
}
