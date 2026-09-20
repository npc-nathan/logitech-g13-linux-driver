//! The G13 protocol, as the device itself describes it.
//!
//! Everything here is derived from the USB HID specification and from bytes and reports the device
//! actually produces. Nothing is derived from another driver's source. See PROVENANCE.md.

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
pub mod descriptor;
pub mod keymap;
pub mod report;

pub use descriptor::{Descriptor, Field, Item, ItemKind, MainItem};
pub use keymap::{
    CONTROL_BITS, Control, bit_for_control, control_for_bit, control_from_name, describe_bit,
};
pub use report::{
    BACKLIGHT_REQUEST, BACKLIGHT_REQUEST_TYPE, BACKLIGHT_VALUE, INPUT_BITS, INPUT_BYTES,
    InputReport, M_KEYS_VALUE, PACKET_BYTES, REPORT_ID_BACKLIGHT, REPORT_ID_INPUT,
    REPORT_ID_M_KEYS, backlight_report, input_bytes_from, m_keys_mask, m_keys_report,
};

/// The vendor and product this crate speaks to.
pub const VENDOR_LOGITECH: u16 = 0x046d;
/// The product id: the G13 gameboard itself.
pub const PRODUCT_G13: u16 = 0xc21c;
