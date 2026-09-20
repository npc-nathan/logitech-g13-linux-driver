//! Read this device's HID report descriptor and decode it.
//!
//! The descriptor is fetched over a control transfer, which the kernel serialises for us and which
//! does not require claiming the interface. That means this runs happily while another driver holds
//! the device  -  which is exactly the situation when replacing one.
//!
//!     cargo run -p g13-device --example descriptor

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use g13_proto::{PRODUCT_G13, VENDOR_LOGITECH, descriptor};
use rusb::UsbContext;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
        .ok_or("no G13 found on USB")?;

    let desc = device.device_descriptor()?;
    let handle = device.open()?;
    let _ = handle.set_auto_detach_kernel_driver(true);

    println!(
        "device: {:04x}:{:04x} USB {:?}",
        desc.vendor_id(),
        desc.product_id(),
        desc.usb_version()
    );

    // GET_DESCRIPTOR(Report), recipient interface, interface 0.
    let mut buffer = vec![0u8; 4096];
    let length = handle
        .read_control(0x81, 0x06, 0x2200, 0, &mut buffer, Duration::from_secs(3))
        .map_err(|error| format!("report descriptor request failed: {error}"))?;
    buffer.truncate(length);
    println!("report descriptor: {} bytes\n", buffer.len());

    for (row, chunk) in buffer.chunks(16).enumerate() {
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
        println!("  {:04x}  {}", row * 16, hex.join(" "));
    }

    let parsed = descriptor::parse(&buffer);
    println!("\n== fields ==");
    for line in parsed.describe() {
        println!("  {line}");
    }
    println!(
        "\nreport 0x01 input:  {} bits ({} bytes)",
        parsed.report_bits(0x01, g13_proto::MainItem::Input),
        parsed
            .report_bits(0x01, g13_proto::MainItem::Input)
            .div_ceil(8)
    );
    Ok(())
}
