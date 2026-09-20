//! What each bit of this device's key report means.
//!
//! **Every entry here was measured.** The report's 56 bits are vendor-defined and the descriptor
//! deliberately does not name them, so the map was walked on the real pad with `g13 watch --map` and
//! confirmed by a second press where a single reading was ambiguous. The control names are what is
//! printed on the hardware, transcribed from it  -  no part of this comes from another driver's
//! configuration or source. See PROVENANCE.md.

/// A control the pad has, as its own legends name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Control {
    /// The round button beside the L keys.
    LeftRound,
    /// The leftmost of the four L keys.
    L1,
    /// The second L key from the left.
    L2,
    /// The third L key from the left.
    L3,
    /// The rightmost of the four L keys.
    L4,
    /// The leftmost M key. The three of them are the keys a profile can be given to by default.
    M1,
    /// The middle M key.
    M2,
    /// The rightmost M key.
    M3,
    /// Macro record.
    MacroRecord,
    /// G keys 1 to 22, left to right across the columns and the run beneath them.
    G(u8),
    /// The two thumb buttons, which report as the left and right mouse buttons.
    LeftMouse,
    /// The right thumb button, which the pad reports as the right mouse button.
    RightMouse,
    /// The stick's click. The stick's direction axes are separate; see `STICK_BITS`.
    StickClick,
}

impl Control {
    /// The name printed on the control, which is also the name a bindings file writes it as.
    pub fn name(&self) -> String {
        match self {
            Control::LeftRound => "LR".to_string(),
            Control::L1 => "L1".to_string(),
            Control::L2 => "L2".to_string(),
            Control::L3 => "L3".to_string(),
            Control::L4 => "L4".to_string(),
            Control::M1 => "M1".to_string(),
            Control::M2 => "M2".to_string(),
            Control::M3 => "M3".to_string(),
            Control::MacroRecord => "MR".to_string(),
            Control::G(number) => format!("G{number}"),
            Control::LeftMouse => "LMB".to_string(),
            Control::RightMouse => "RMB".to_string(),
            Control::StickClick => "JCLICK".to_string(),
        }
    }
}

/// Every discrete control and the bit it sets. Measured, one bit each, every one confirmed by a press
/// that produced exactly two changes  -  down and up.
pub const CONTROL_BITS: &[(Control, usize)] = &[
    (Control::LeftRound, 40),
    (Control::L1, 41),
    (Control::L2, 42),
    (Control::L3, 43),
    (Control::L4, 44),
    (Control::M1, 45),
    (Control::M2, 46),
    (Control::M3, 47),
    (Control::MacroRecord, 48),
    (Control::LeftMouse, 49),
    (Control::RightMouse, 50),
    (Control::StickClick, 51),
    (Control::G(1), 16),
    (Control::G(2), 17),
    (Control::G(3), 18),
    (Control::G(4), 19),
    (Control::G(5), 20),
    (Control::G(6), 21),
    (Control::G(7), 22),
    (Control::G(8), 23),
    (Control::G(9), 24),
    (Control::G(10), 25),
    (Control::G(11), 26),
    (Control::G(12), 27),
    (Control::G(13), 28),
    (Control::G(14), 29),
    (Control::G(15), 30),
    (Control::G(16), 31),
    (Control::G(17), 32),
    (Control::G(18), 33),
    (Control::G(19), 34),
    (Control::G(20), 35),
    (Control::G(21), 36),
    (Control::G(22), 37),
];

/// Bits the stick drives. Two bytes are involved rather than one bit per direction, so how they encode
/// a position is **not established** and nothing here claims otherwise; the walk reports the ranges each
/// byte spans, and the encoding will be settled by using the stick.
pub const STICK_BITS: std::ops::RangeInclusive<usize> = 0..=15;

/// Bits that are device state rather than input: 39 is set in reports the stick produces, 55 toggles
/// between reports. Neither is a control and neither may be treated as a key.
pub const FLAG_BITS: &[usize] = &[39, 55];

/// Bits that never responded to any control during the walk. Not pressed, or unused by this pad  -
/// recorded so the difference between "no" and "not asked" stays visible.
pub const UNSEEN_BITS: &[usize] = &[38, 52, 53, 54];

/// The control a bit belongs to, if it is a discrete control.
pub fn control_for_bit(bit: usize) -> Option<Control> {
    CONTROL_BITS
        .iter()
        .find(|(_, candidate)| *candidate == bit)
        .map(|(control, _)| *control)
}

/// The control a name refers to, as the pad's legends spell it: `LR`, `L1`, `M1`, `G7`, `LMB`.
///
/// The inverse of [`Control::name`], so that a binding can name a control and be resolved to its bit.
pub fn control_from_name(name: &str) -> Option<Control> {
    let wanted = name.trim();
    CONTROL_BITS
        .iter()
        .map(|(control, _)| *control)
        .find(|control| control.name().eq_ignore_ascii_case(wanted))
}

/// The bit a control sets.
pub fn bit_for_control(control: Control) -> Option<usize> {
    CONTROL_BITS
        .iter()
        .find(|(candidate, _)| *candidate == control)
        .map(|(_, bit)| *bit)
}

/// A readable name for a bit, whichever kind it is.
pub fn describe_bit(bit: usize) -> String {
    if let Some(control) = control_for_bit(bit) {
        control.name()
    } else if STICK_BITS.contains(&bit) {
        format!("stick bit {bit}")
    } else if FLAG_BITS.contains(&bit) {
        format!("flag {bit}")
    } else {
        format!("bit {bit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn the_g_keys_are_a_contiguous_run_of_twenty_two() {
        for number in 1..=22u8 {
            assert_eq!(
                bit_for_control(Control::G(number)),
                Some(15 + number as usize)
            );
        }
    }

    #[test]
    fn every_control_has_its_own_bit() {
        let bits: HashSet<usize> = CONTROL_BITS.iter().map(|(_, bit)| *bit).collect();
        assert_eq!(bits.len(), CONTROL_BITS.len(), "no bit is claimed twice");
        assert_eq!(CONTROL_BITS.len(), 34, "34 discrete controls");
    }

    #[test]
    fn the_measured_bits_are_where_the_walk_found_them() {
        assert_eq!(bit_for_control(Control::LeftRound), Some(40));
        assert_eq!(bit_for_control(Control::L1), Some(41));
        assert_eq!(bit_for_control(Control::L4), Some(44));
        assert_eq!(bit_for_control(Control::M1), Some(45));
        assert_eq!(bit_for_control(Control::MacroRecord), Some(48));
        assert_eq!(bit_for_control(Control::LeftMouse), Some(49));
        assert_eq!(bit_for_control(Control::RightMouse), Some(50));
        assert_eq!(bit_for_control(Control::StickClick), Some(51));
    }

    #[test]
    fn the_flag_bits_are_not_controls() {
        for bit in FLAG_BITS {
            assert!(
                control_for_bit(*bit).is_none(),
                "flag {bit} must not be a control"
            );
        }
    }

    #[test]
    fn a_name_resolves_back_to_its_control_and_bit() {
        assert_eq!(control_from_name("G7"), Some(Control::G(7)));
        assert_eq!(
            control_from_name("g7"),
            Some(Control::G(7)),
            "case does not matter"
        );
        assert_eq!(bit_for_control(control_from_name("LR").unwrap()), Some(40));
        assert_eq!(control_from_name("LMB"), Some(Control::LeftMouse));
        assert_eq!(control_from_name("nonsense"), None);
    }

    #[test]
    fn names_round_trip_readably() {
        assert_eq!(Control::LeftRound.name(), "LR");
        assert_eq!(Control::G(7).name(), "G7");
        assert_eq!(describe_bit(16), "G1");
        assert_eq!(describe_bit(0), "stick bit 0");
        assert_eq!(describe_bit(55), "flag 55");
        assert_eq!(describe_bit(52), "bit 52");
    }
}
