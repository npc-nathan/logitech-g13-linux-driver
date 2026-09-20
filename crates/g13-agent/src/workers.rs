//! Every thread the driver runs, and what ends it.
//!
//! There are two kinds, and the difference is whether anything has to stop them.
//!
//! - **For the life of the driver**, one per job, owned here: the screen's two threads - one gathering the
//!   readings an applet or a machine visual wants, one drawing - and the stick's one. Dropping `Workers` stops
//!   and joins all three, which is the reason this struct exists at all: a thread nobody owns is a thread
//!   nobody stops, and the pad's uinput devices are only handed back once the stick's thread has returned.
//! - **Started by a press and ending by itself**: a macro played (`macro_report` in `lib.rs`), a screen's
//!   command run (`select_report`), a list read (`ListWatch::pending`), and the record wizard's key capture
//!   (`g13_device::capture::record_keys`). Each answers on a channel the loop holds, and each ends when it is
//!   done; there is nothing to join, and dropping the channel is how the loop says it no longer wants the
//!   answer.
//!
//! So "what is running" is a question with an answer in one file: the three below, and those four.

use crate::stick::StickWorker;
use crate::{Bindings, ScreenWorker};

/// The threads that run for as long as the driver does.
///
/// Field order is the drop order - the screen first, then the stick - and each of them joins its own threads as
/// it goes. The set is what makes that one thing to hold: two locals would be two places to remember, and the
/// next worker added belongs here rather than wherever it happens to be started.
pub struct Workers {
    /// The screen: readings on one thread, drawing on another. Started once the visual is known.
    pub screen: ScreenWorker,
    /// The stick: one thread, and only while the mode in force needs a pointer or a gamepad.
    pub stick: StickWorker,
}

impl Workers {
    /// Tell the stick what the map that just came into force needs: a pointer, a gamepad, or neither.
    ///
    /// One function because the three places a map arrives - startup, a profile switch, and an edit to the
    /// file - all have to say it, and the answer is the map's own (`needs_mouse`, `needs_gamepad`) rather than
    /// the caller's.
    pub fn set_needs(&self, bindings: &Bindings) {
        self.stick
            .set_needs(bindings.needs_mouse(), bindings.needs_gamepad());
    }
}
