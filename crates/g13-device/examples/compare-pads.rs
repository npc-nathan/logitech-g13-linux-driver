//! Compare this build's virtual gamepad with a real one, axis by axis and button by button.
//!
//!   cargo run -p g13-device --example compare-pads --release -- /dev/input/event30
//!
//! It creates the gamepad this build presents and reads both devices' own descriptions, so the answer to
//! "is the d-pad the same shape as a real one" is measured from the two devices rather than assumed. The real
//! pad's description is the specification: a game reads it, not ours.

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use evdev::{AbsoluteAxisCode, Device, KeyCode};

/// The `/dev/input/eventN` node inside a device's sysfs directory, unless it has none yet.
fn node_for(syspath: &std::path::Path) -> Option<String> {
    for entry in std::fs::read_dir(syspath).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("event") {
            return Some(format!("/dev/input/{name}"));
        }
    }
    None
}

/// Open a device, waiting briefly for udev to finish with it.
fn open_waiting(path: &str) -> Option<Device> {
    for _ in 0..40 {
        if let Ok(device) = Device::open(path) {
            return Some(device);
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    None
}

/// Open a node and describe it, for a person reading the two pads side by side.
fn describe(label: &str, path: &str) -> Device {
    let device = Device::open(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    describe_device(label, path, device)
}

/// Print what an open device says about itself and hand it back, which is the measurement this tool exists to
/// take.
fn describe_device(label: &str, path: &str, device: Device) -> Device {
    let id = device.input_id();
    println!("\n=== {label} ===");
    println!("  {path}");
    println!("  name        {}", device.name().unwrap_or("(none)"));
    println!(
        "  identity    bus {:#06x} vendor {:#06x} product {:#06x} version {:#06x}",
        id.bus_type().0,
        id.vendor(),
        id.product(),
        id.version()
    );
    println!(
        "  event types {:?}",
        device.supported_events().iter().collect::<Vec<_>>()
    );
    println!(
        "  properties  {:?}",
        device.properties().iter().collect::<Vec<_>>()
    );

    match device.supported_keys() {
        Some(keys) => {
            let mut named: Vec<String> = keys.iter().map(|k| format!("{k:?}")).collect();
            named.sort();
            println!("  buttons     {}: {}", named.len(), named.join(" "));
        }
        None => println!("  buttons     none"),
    }

    match device.get_absinfo() {
        Ok(axes) => {
            let mut lines: Vec<String> = axes
                .map(|(code, info)| {
                    format!(
                        "{:?} min {} max {} fuzz {} flat {} res {}",
                        code,
                        info.minimum(),
                        info.maximum(),
                        info.fuzz(),
                        info.flat(),
                        info.resolution()
                    )
                })
                .collect();
            lines.sort();
            println!("  axes        {}", lines.len());
            for line in lines {
                println!("      {line}");
            }
        }
        Err(e) => println!("  axes        could not be read: {e}"),
    }
    device
}

fn main() {
    let real_path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: compare-pads <the real pad's event node, e.g. /dev/input/event30>");
        std::process::exit(2);
    });

    // the gamepad this build presents, made here so its own node is known
    let mut ours = g13_device::joystick::VirtualStick::new().expect("a virtual gamepad");
    let ours_path = ours
        .syspath()
        .as_deref()
        .and_then(node_for)
        .expect("the kernel gave our device a path");
    println!("ours: {ours_path}");

    // A device node created a moment ago may not have its access control entry yet: udev grants it, and this
    // can get there first. Opening it once and giving up reports a permissions problem for a device that is
    // readable a moment later, so wait for it - and if it never works, say what the node's permissions are
    // rather than leaving a bare "denied".
    let ours_device = match open_waiting(&ours_path) {
        Some(device) => describe_device("ours (g13 stick)", &ours_path, device),
        None => {
            println!("\n=== ours (g13 stick) ===");
            println!("  {ours_path} could not be opened");
            if let Ok(meta) = std::fs::metadata(&ours_path) {
                use std::os::unix::fs::PermissionsExt;
                println!("  mode {:o}", meta.permissions().mode());
            }
            match std::process::Command::new("getfacl")
                .arg("-p")
                .arg(&ours_path)
                .output()
            {
                Ok(out) => println!(
                    "  {}",
                    String::from_utf8_lossy(&out.stdout)
                        .trim()
                        .replace('\n', "\n  ")
                ),
                Err(e) => println!("  (getfacl: {e})"),
            }
            println!(
                "  A game opening this node needs the same access. If it is missing here it is missing there."
            );
            println!(
                "  The rule is packaging/70-g13-record.rules; reinstall with udevadm control --reload."
            );
            std::process::exit(1);
        }
    };
    let real_device = describe("real", &real_path);

    // and the part worth reading: what is different
    println!("\n=== the differences ===");
    let ours_axes: Vec<AbsoluteAxisCode> = ours_device
        .get_absinfo()
        .map(|it| it.map(|(c, _)| c).collect())
        .unwrap_or_default();
    let real_axes: Vec<AbsoluteAxisCode> = real_device
        .get_absinfo()
        .map(|it| it.map(|(c, _)| c).collect())
        .unwrap_or_default();
    for axis in &ours_axes {
        if !real_axes.contains(axis) {
            println!("  ours has {axis:?}, the real pad does not");
        }
    }
    for axis in &real_axes {
        if !ours_axes.contains(axis) {
            println!("  the real pad has {axis:?}, ours does not  <-- a game may read this");
        }
    }
    // ranges matter as much as presence: a hat of the wrong size or a trigger with the wrong travel reads
    // as a stuck or dead axis
    // AbsoluteAxisCode is not Ord, so these are looked up by equality rather than in a tree
    let ours_info: Vec<(AbsoluteAxisCode, evdev::AbsInfo)> = ours_device
        .get_absinfo()
        .map(|it| it.collect())
        .unwrap_or_default();
    let real_info: Vec<(AbsoluteAxisCode, evdev::AbsInfo)> = real_device
        .get_absinfo()
        .map(|it| it.collect())
        .unwrap_or_default();
    for (axis, ours) in &ours_info {
        if let Some((_, real)) = real_info.iter().find(|(code, _)| code == axis)
            && (ours.minimum() != real.minimum()
                || ours.maximum() != real.maximum()
                || ours.flat() != real.flat())
        {
            println!(
                "  {axis:?}: ours {}..{} flat {}, real {}..{} flat {}",
                ours.minimum(),
                ours.maximum(),
                ours.flat(),
                real.minimum(),
                real.maximum(),
                real.flat()
            );
        }
    }
    let ours_keys: Vec<KeyCode> = ours_device
        .supported_keys()
        .map(|k| k.iter().collect())
        .unwrap_or_default();
    let real_keys: Vec<KeyCode> = real_device
        .supported_keys()
        .map(|k| k.iter().collect())
        .unwrap_or_default();
    for key in &ours_keys {
        if !real_keys.contains(key) {
            println!("  ours has {key:?}, the real pad does not");
        }
    }
    for key in &real_keys {
        if !ours_keys.contains(key) {
            println!("  the real pad has {key:?}, ours does not  <-- a game may read this");
        }
    }
    println!("(nothing listed above means the two devices describe themselves the same way)");

    drop(ours);
}
