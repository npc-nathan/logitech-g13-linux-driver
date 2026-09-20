//! Draw a frame and print it as text, so a layout can be checked without the pad.
//!
//!     cargo run -p g13-screen --example render -- "hello" "second line"
//!
//! Uses the same Debug rendering a failing test prints, which is the point: what you see here is what the
//! framebuffer holds.

// a tool run by hand against fixtures: it prints what it finds and unwraps what it needs, which is
// exactly what the library may not do
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
use g13_screen::{Frame, VISIBLE_HEIGHT, WIDTH};

fn main() {
    let lines: Vec<String> = std::env::args().skip(1).collect();
    let mut frame = Frame::new();
    if lines.is_empty() {
        frame.text_line(0, "g13 screen");
        frame.text_line(2, "160x43, five rows");
        frame.fill(0, VISIBLE_HEIGHT - 1, WIDTH, 1, true);
    } else {
        for (index, line) in lines.iter().enumerate() {
            frame.text_line(index, line);
        }
    }
    println!("{frame:?}");
    println!(
        "report: {} bytes, id 0x{:02x}",
        frame.to_report().len(),
        frame.to_report()[0]
    );
}
