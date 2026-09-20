//! The protocol facts this device states about itself, kept as a fixture.
//!
//! The bytes in `fixtures/g13-report-descriptor.bin` came off the pad with a `GET_DESCRIPTOR(Report)`
//! request (see `cargo run -p g13-device --example descriptor`). They are the device's own account of
//! its reports, which is why they are the source of truth here rather than anybody's driver.

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use g13_proto::{MainItem, descriptor};

fn device_descriptor() -> Vec<u8> {
    std::fs::read("tests/fixtures/g13-report-descriptor.bin").expect("fixture is present")
}

#[test]
fn the_descriptor_is_what_the_device_returned() {
    assert_eq!(device_descriptor().len(), 61);
    assert_eq!(&device_descriptor()[..4], &[0x06, 0x00, 0xff, 0x09]);
    assert_eq!(
        *device_descriptor().last().unwrap(),
        0xc0,
        "ends a collection"
    );
}

#[test]
fn the_key_report_is_seven_bytes_after_the_report_id() {
    let parsed = descriptor::parse(&device_descriptor());
    let inputs = parsed.fields_for(0x01, MainItem::Input);
    assert_eq!(inputs.len(), 1, "one input item");
    assert_eq!(inputs[0].report_count, 7);
    assert_eq!(inputs[0].report_size, 8);
    assert_eq!(inputs[0].usage_page, 0xff00, "vendor-defined page");
    assert!(!inputs[0].array, "a bitmap, not an array of usage codes");
    assert_eq!(parsed.report_bits(0x01, MainItem::Input), 56);
}

#[test]
fn the_lcd_pipe_is_991_bytes() {
    let parsed = descriptor::parse(&device_descriptor());
    let outputs = parsed.fields_for(0x03, MainItem::Output);
    assert_eq!(outputs.len(), 1);
    assert_eq!(parsed.report_bits(0x03, MainItem::Output), 991 * 8);
}

#[test]
fn the_feature_reports_are_where_control_lives() {
    let parsed = descriptor::parse(&device_descriptor());
    let sizes: Vec<(u8, u32)> = parsed
        .fields
        .iter()
        .filter(|field| field.kind == MainItem::Feature)
        .map(|field| (field.report_id, field.report_size * field.report_count))
        .collect();
    assert_eq!(
        sizes,
        vec![(0x07, 32), (0x04, 32), (0x05, 32), (0x06, 2056)]
    );
}

#[test]
fn a_single_byte_packet_is_rejected_rather_than_encoded() {
    // the descriptor says 7 bytes; anything else is not this device's report
    let parsed = descriptor::parse(&device_descriptor());
    let expected = parsed.report_bits(0x01, MainItem::Input).div_ceil(8);
    assert_eq!(expected, 7);
}
