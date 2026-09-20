//! How evenly does a moving screen actually get drawn?
//!
//! There is a slight pulse of speed change between three and five times a second on a media applet: very minor
//! but noticeable, and not smooth. The spinner is smooth; the **scrolling text** pulses in speed.
//!
//! A scroll's position is `clock x speed` - absolute, not accumulated - so the position is right for the moment it
//! is drawn, whatever the moment is. What the eye judges is the *step* between frames: at 15 pixels a second and
//! twenty drawings a second that is 0.75 pixels, and if the period wanders between 30ms and 60ms the steps wander
//! between 0.45 and 0.9 - which reads as the speed pulsing while the average stays right.
//!
//! So this measures the period, with a wake signal at the rate the driver sends one, which is the case a media
//! applet sits in. Run with a copy of the config directory elsewhere:
//!
//!   cargo run -p g13-agent --example frame-period --release -- /tmp/g13-frames applet:media-controller

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use g13_agent::{ScreenWake, ScreenWorker};
use std::time::{Duration, Instant};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap_or_else(|| "/tmp/g13-frames".to_string());
    let visual = args
        .next()
        .unwrap_or_else(|| "applet:media-controller".to_string());
    let dir = std::path::PathBuf::from(dir);

    let wake = ScreenWake::default();
    let worker = ScreenWorker::start(
        visual.clone(),
        dir.join("applets"),
        dir.clone(),
        // no writer: this is about the drawing, not about the panel
        None,
        wake.clone(),
    );

    // The driver wakes the screen only when the pad's own state changes - which profile, which screen, whether a
    // recording is running - so while a track plays and nobody touches the pad, nothing wakes it and it draws on
    // its own timeout. That is the case measured here. Set WAKE_MS to signal at
    // some other rate to see what a wake does.
    let publishing = match std::env::var("WAKE_MS")
        .ok()
        .and_then(|ms| ms.parse::<u64>().ok())
    {
        None => None,
        Some(ms) => {
            let wake = wake.clone();
            Some(std::thread::spawn(move || {
                while !STOP.load(std::sync::atomic::Ordering::Relaxed) {
                    wake.wake();
                    std::thread::sleep(Duration::from_millis(ms));
                }
            }))
        }
    };

    let started = Instant::now();
    let mut last = started;
    let mut periods = Vec::new();
    while started.elapsed() < Duration::from_secs(4) {
        if worker.take().is_some() {
            periods.push(last.elapsed().as_secs_f64() * 1000.0);
            last = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    STOP.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Some(publishing) = publishing {
        let _ = publishing.join();
    }

    if periods.is_empty() {
        println!("no frames");
        return;
    }
    let mut sorted = periods.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];
    let mean: f64 = periods.iter().sum::<f64>() / periods.len() as f64;
    println!("{visual}: {} frames in four seconds", periods.len());
    println!(
        "period: median {median:.1}ms, mean {mean:.1}ms, fastest {:.1}ms, slowest {:.1}ms",
        sorted[0],
        sorted[sorted.len() - 1]
    );
    // the spread is the point: a steady drawing has a tight one, and a pulsing one does not
    let wobble = sorted[sorted.len() - 1] - sorted[0];
    println!("spread: {wobble:.1}ms");
    // how far apart consecutive periods are, which is what the eye sees as speed changing
    let steps: Vec<f64> = periods
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .collect();
    let biggest = steps.iter().cloned().fold(0.0f64, f64::max);
    let over = steps.iter().filter(|step| **step > 8.0).count();
    println!(
        "change between one frame and the next: biggest {biggest:.1}ms, {over} of {} over 8ms",
        steps.len()
    );
    // and what that means for a scroll at the speed a title runs at
    let speed = 15.0;
    println!(
        "at {speed:.0} pixels a second, a step of {:.0}ms is {:.2} pixels and one of {:.0}ms is {:.2}",
        median,
        speed * median / 1000.0,
        sorted[sorted.len() - 1],
        speed * sorted[sorted.len() - 1] / 1000.0
    );
}

/// Set when the four seconds are up, so a waking thread stops rather than sleeping through the join.
static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
