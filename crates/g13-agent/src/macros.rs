//! Playing a macro: key downs, key ups and pauses, in the order the file lists them.
//!
//! This runs on its own thread, because a macro takes as long as its delays say and the pad has to keep being
//! read while it does - which is also why the keyboard is shared rather than owned here.
//!
//! The waiting is by deadline rather than by sleeping between steps: the time a key takes to send comes out of
//! the interval it belongs to instead of being added to it, so a long macro does not drift away from the rhythm
//! it was recorded at.

use g13_device::keyboard::{self, KeyCode};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Play a macro: key downs, key ups and pauses in the order the file lists them.
///
/// Runs on its own thread, because a macro takes as long as its delays say and the pad must keep being read
/// while it does. The keyboard is shared for that reason.
pub fn play_macro(
    keyboard: &Arc<Mutex<keyboard::VirtualKeyboard>>,
    macro_file: &g13_config::Macro,
    repeats: u32,
) -> Result<(), String> {
    play_macro_reporting(keyboard, macro_file, repeats, |_| {})
}

/// Play a macro, calling `on_step` for each step as it is performed.
///
/// The callback exists so the steps this program *performed* can be compared with the events the system
/// received. A macro that silently does nothing and a macro whose events go missing look identical
/// otherwise.
pub fn play_macro_reporting<F>(
    keyboard: &Arc<Mutex<keyboard::VirtualKeyboard>>,
    macro_file: &g13_config::Macro,
    repeats: u32,
    mut on_step: F,
) -> Result<(), String>
where
    F: FnMut(&g13_config::MacroStep),
{
    // A pause at the very start is the hand moving to the first key, not part of the pattern: the trigger is
    // where the macro starts. A pause *between* steps is the pattern, and it is kept exactly.
    let steps: Vec<g13_config::MacroStep> = match macro_file.steps.first() {
        Some(g13_config::MacroStep::Delay(_)) => macro_file.steps[1..].to_vec(),
        _ => macro_file.steps.clone(),
    };
    let due = g13_config::step_deadlines(&steps);
    for _ in 0..repeats.max(1) {
        let started = std::time::Instant::now();
        for (step, at) in steps.iter().zip(&due) {
            on_step(step);
            // Wait until this step is due rather than after the work of the one before it: the cost of
            // sending a key comes out of its interval instead of being added to it.
            if let Some(rest) = at.checked_sub(started.elapsed()) {
                std::thread::sleep(rest);
            }
            match step {
                g13_config::MacroStep::KeyDown(code) => {
                    let mut keyboard = keyboard.lock().map_err(|_| "keyboard lock poisoned")?;
                    keyboard
                        .press(KeyCode::new(*code))
                        .map_err(|e| e.to_string())?;
                }
                g13_config::MacroStep::KeyUp(code) => {
                    let mut keyboard = keyboard.lock().map_err(|_| "keyboard lock poisoned")?;
                    keyboard
                        .release(KeyCode::new(*code))
                        .map_err(|e| e.to_string())?;
                }
                g13_config::MacroStep::Delay(ms) => {
                    std::thread::sleep(Duration::from_millis(*ms as u64));
                }
            }
        }
    }
    Ok(())
}
