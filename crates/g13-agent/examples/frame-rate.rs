//! How fast can a frame actually be put on the pad's screen?
//!
//! The driver's rate cap was a conservative guess for a while - 20/s, with the device's own ceiling unmeasured.
//! The number decides whether anything on the screen can be smoother than it is, so it was measured rather than
//! guessed at, and this is the instrument.
//!
//! The pad must be free. Stop `g13 run` (or the service, or `g13-visuals`) first, and stop it the way it is
//! restarted afterwards, because a pad with no driver is not detected at all.
//!
//! Run: cargo run -p g13-device --example frame-rate --release
//!
//! The bar sweeping across the panel is not decoration: every frame has to differ, or a device that skips
//! unchanged frames would measure faster than it is.

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use g13_device::Device;
use g13_screen::Frame;
use std::time::{Duration, Instant};

/// Frames written back to back, which is the throughput rather than a sustained rate.
const FRAMES: usize = 400;

fn main() {
    let device = match Device::open() {
        Ok(device) => device,
        Err(error) => {
            println!("the pad could not be taken: {error}");
            println!(
                "stop whatever is driving it first  -  `g13 run`, the service, or `g13-visuals`."
            );
            std::process::exit(1);
        }
    };

    let mut writes = Vec::with_capacity(FRAMES);
    let mut accepted = 0usize;
    for at in 0..FRAMES {
        let mut frame = Frame::new();
        let x = at % g13_screen::WIDTH;
        frame.fill(x, 0, 6, g13_screen::VISIBLE_HEIGHT, true);
        frame.text(2, 30, &format!("{at:>4}"));
        let report = frame.to_report();
        let start = Instant::now();
        match device.write_lcd(&report) {
            Ok(written) if written == report.len() => accepted += 1,
            Ok(written) => {
                println!("short write: {written} of {} bytes", report.len());
            }
            Err(error) => {
                println!("write failed after {at} frames: {error}");
                break;
            }
        }
        writes.push(start.elapsed().as_secs_f64());
    }

    if writes.is_empty() {
        println!("nothing was written");
        std::process::exit(1);
    }
    let total: f64 = writes.iter().sum();
    let mut sorted = writes.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let at = |share: f64| sorted[((sorted.len() - 1) as f64 * share) as usize];
    println!(
        "{accepted} of {FRAMES} frames written, {:.1} bytes each",
        Frame::new().to_report().len() as f64
    );
    println!(
        "total {:.3}s -> {:.1} frames a second back to back",
        total,
        accepted as f64 / total
    );
    println!(
        "one write: median {:.2}ms, p99 {:.2}ms, worst {:.2}ms, best {:.2}ms",
        at(0.5) * 1000.0,
        at(0.99) * 1000.0,
        sorted[sorted.len() - 1] * 1000.0,
        sorted[0] * 1000.0
    );

    // And a sustained second at a rate something could actually ask for, since throughput is not the same
    // question as "can it keep this up while the driver is also reading the pad".
    for rate in [20u32, 30, 60] {
        let period = Duration::from_secs_f64(1.0 / f64::from(rate));
        let until = Instant::now() + Duration::from_secs(2);
        let mut sent = 0u32;
        let mut late = 0u32;
        let mut next = Instant::now();
        while Instant::now() < until {
            let mut frame = Frame::new();
            let x = (sent as usize * 7) % g13_screen::WIDTH;
            frame.fill(x, 0, 6, g13_screen::VISIBLE_HEIGHT, true);
            let report = frame.to_report();
            let start = Instant::now();
            if device.write_lcd(&report).is_ok() {
                sent += 1;
            }
            if start.duration_since(next) > period {
                late += 1;
            }
            next += period;
            let spent = start.elapsed();
            if spent < period {
                std::thread::sleep(period - spent);
            }
        }
        println!("asked for {rate}/s for two seconds: sent {sent}, {late} of them late");
    }

    // leave it blank rather than showing whatever the last made-up frame was
    let _ = device.write_lcd(&Frame::new().to_report());
    println!("done; the panel is blank and the pad is free for `g13 run` again");
}
