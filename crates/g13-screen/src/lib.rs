//! The 160x43 framebuffer, the font, text, and the rule that keeps ink off the pictures.
//!
//! The device stores eight *vertical* pixels per byte: the byte for column `x` in the band of rows
//! `y-(y%8) .. y-(y%8)+7` lives at `x + (y/8)*160`, and bit `y%8` of it is that pixel. That layout comes from
//! the published G13 protocol, and it agrees with this device's own report descriptor, which says the LCD
//! report is 991 bytes: 31 bytes of header plus 960 of image (160 columns x 48 rows / 8).
//!
//! The panel shows 43 of those 48 rows. The last five are written and never seen, so anything drawn there is
//! invisible rather than wrong  -  but text is laid out against `VISIBLE_HEIGHT` so that never happens by
//! accident.

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
pub mod font;

use font::ADVANCE;

/// How far the pen moves after a character, in pixels.
pub const ADVANCE_COLUMNS: usize = ADVANCE;

/// Columns of the panel.
pub const WIDTH: usize = 160;
/// Rows the panel actually shows.
pub const VISIBLE_HEIGHT: usize = 43;
/// Rows the device stores, five of which are not visible.
pub const HEIGHT: usize = 48;
/// One byte per column, so a band of eight rows is one byte per column across the width.
const BYTES: usize = WIDTH * HEIGHT / 8;
/// The whole transfer: report id, then the header, then the image.
pub const REPORT_ID: u8 = 0x03;
/// Bytes the report carries before the image: the header the device's own LCD command is written into.
const HEADER: usize = 31;
/// The whole transfer: report id, header, then the image.
pub const REPORT_LEN: usize = 1 + HEADER + BYTES;
/// Rows of text that fit on the visible part.
pub const TEXT_ROWS: usize = VISIBLE_HEIGHT / font::LINE_HEIGHT;
/// Characters that fit across.
pub const TEXT_COLUMNS: usize = WIDTH / ADVANCE;

/// The 160x48 monochrome image the device holds. A set bit is ink.
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    /// The image: one byte per column, eight vertical pixels in each, a set bit being ink.
    bits: [u8; BYTES],
    /// Where drawing is allowed while this is set. A widget that runs its text has to stop at the width it was
    /// given, and `set` alone only knows the panel's own edges: without this, a long title would run across
    /// whatever is drawn beside it. Cleared again by `unclipped`, so ordinary drawing never notices it.
    clip: Option<(usize, usize, usize, usize)>,
    /// The fonts text is drawn in, in order: every character goes to the first one that has a glyph for it.
    ///
    /// It lives on the frame rather than travelling as an argument to every call, so the whole drawing path - plain
    /// text, text that lets a picture show through, running text, inverted text - uses one stack, and a screen that
    /// wants a different one sets it once.
    fonts: Vec<font::Font>,
}

impl Default for Frame {
    fn default() -> Self {
        Self::new()
    }
}

impl Frame {
    /// A blank frame: nothing drawn, which is what the device calls black.
    pub fn new() -> Self {
        Self {
            bits: [0; BYTES],
            clip: None,
            fonts: vec![font::panel().clone()],
        }
    }

    /// Draw this frame's text in a different stack of fonts - the panel font with a wider one behind it, say.
    pub fn with_fonts(mut self, fonts: Vec<font::Font>) -> Self {
        self.set_fonts(fonts);
        self
    }

    /// The same, on a frame that already exists.
    pub fn set_fonts(&mut self, fonts: Vec<font::Font>) {
        if !fonts.is_empty() {
            self.fonts = fonts;
        }
    }

    /// Which font draws this character, and what it draws.
    fn glyph_here(&self, character: char) -> (&font::Font, [u8; 8]) {
        let (font, ink, _) = font::in_stack(&self.fonts, character);
        (font, ink)
    }

    /// Draw only within this rectangle until `unclipped`. The panel's own edges still apply.
    pub fn clipped(&mut self, x: usize, y: usize, width: usize, height: usize) {
        self.clip = Some((x, y, x.saturating_add(width), y.saturating_add(height)));
    }

    /// Draw anywhere on the panel again.
    pub fn unclipped(&mut self) {
        self.clip = None;
    }

    /// Whether a pixel is inside the clip, if there is one.
    fn inside(&self, x: usize, y: usize) -> bool {
        match self.clip {
            None => true,
            Some((left, top, right, bottom)) => x >= left && x < right && y >= top && y < bottom,
        }
    }

    /// Blank everything.
    pub fn clear(&mut self) {
        self.bits = [0; BYTES];
    }

    /// Where the byte holding pixel `(x, y)` sits in `bits`, or none when the coordinate is off the panel.
    fn offset(x: usize, y: usize) -> Option<usize> {
        if x >= WIDTH || y >= HEIGHT {
            return None;
        }
        Some(x + (y / 8) * WIDTH)
    }

    /// Turn one pixel on or off. Out-of-range coordinates are ignored rather than wrapping.
    pub fn set(&mut self, x: usize, y: usize, ink: bool) {
        if !self.inside(x, y) {
            return;
        }
        if let Some(offset) = Self::offset(x, y) {
            let mask = 1u8 << (y % 8);
            if ink {
                self.bits[offset] |= mask;
            } else {
                self.bits[offset] &= !mask;
            }
        }
    }

    /// Whether one pixel is ink.
    pub fn get(&self, x: usize, y: usize) -> bool {
        match Self::offset(x, y) {
            Some(offset) => self.bits[offset] & (1u8 << (y % 8)) != 0,
            None => false,
        }
    }

    /// Move the whole picture sideways, dropping what goes off an edge.
    ///
    /// A theme's glitch is this: the frame drawn for one moment shifted by a pixel or two, so the picture looks
    /// like it slipped rather than like it wobbled.
    pub fn shift(&mut self, by: i64) {
        if by == 0 {
            return;
        }
        let before = self.clone();
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let from = x as i64 - by;
                let ink = (0..WIDTH as i64).contains(&from) && before.get(from as usize, y);
                self.set(x, y, ink);
            }
        }
    }

    /// Turn a rectangle inside out: ink becomes a hole, blank becomes ink.
    ///
    /// This is what a widget looks like when an alert inverts it - a filled box with its own drawing punched out
    /// - and it needs nothing to know about the widget it is drawn over.
    pub fn invert_rect(&mut self, x: usize, y: usize, width: usize, height: usize) {
        for row in y..y + height {
            for column in x..x + width {
                let was = self.get(column, row);
                self.set(column, row, !was);
            }
        }
    }

    /// Turn a rectangle on or off.
    pub fn fill(&mut self, x: usize, y: usize, width: usize, height: usize, ink: bool) {
        for row in y..y + height {
            for column in x..x + width {
                self.set(column, row, ink);
            }
        }
    }

    /// The same, for text about to be drawn on this frame: the frame knows which fonts it is using.
    pub fn line_height(&self, text: &str) -> usize {
        Self::line_height_in(&self.fonts, text)
    }

    /// How tall a line of this text is: the tallest font it actually used.
    ///
    /// A line does not have a height of its own, it has the height of what is in it. A 12x12 Chinese character on a
    /// 6x8 screen makes *that line* twelve rows tall and everything below it moves down; clipping it would be the
    /// easier answer and a worse one, because it would hide the character the writer asked for. Text that uses only
    /// the panel font is eight rows, so nothing that exists today changes.
    pub fn line_height_in(fonts: &[font::Font], text: &str) -> usize {
        let mut height = 0;
        for character in text.chars() {
            let (font, _, _) = font::in_stack(fonts, character);
            height = height.max(font.height);
        }
        // only what was drawn counts: a stack headed by a tall font does not make every line tall, it makes the lines
        // that *use* it tall. An empty line still takes one line, which is the height of the font it would have used.
        match height {
            0 => fonts.first().map(|font| font.height).unwrap_or(0),
            height => height,
        }
    }

    /// Draw text with the top-left of the first character at `(x, y)`, clipped at the edges.
    pub fn text(&mut self, x: usize, y: usize, text: &str) {
        let _ = self.text_in(&[font::panel().clone()], x, y, text);
    }

    /// The same, in a **stack of fonts**: every character is drawn by the first font that has a glyph for it, and
    /// measured by *that* font.
    ///
    /// This is what lets one string hold more than one script - a Japanese song title inside an English applet, which
    /// is what `media_title` can hold - and it is why the advance is asked for rather than taken from a constant: a 12x12
    /// font fits thirteen characters a line where this one fits twenty-six. Returns the width drawn.
    pub fn text_in(&mut self, fonts: &[font::Font], x: usize, y: usize, text: &str) -> usize {
        let mut pen = x;
        for character in text.chars() {
            let (font, ink, _) = font::in_stack(fonts, character);
            for (column, byte) in ink.iter().enumerate() {
                for row in 0..font.rows {
                    if byte & (1 << row) != 0 {
                        // a pixel outside the panel is dropped, never wrapped onto the next line
                        self.set(pen + column, y + row, true);
                    }
                }
            }
            pen += font.advance;
        }
        pen - x
    }

    /// The same as `text_where_blank`, starting from a signed pen so a character may be half off the left.
    ///
    /// A running text moves a few pixels at a time rather than a whole character, so its first character is
    /// usually part-way out of the box it is drawn in. Columns left of the panel are dropped, not wrapped.
    pub fn text_where_blank_from(&mut self, x: isize, y: usize, text: &str) -> usize {
        let mut pen = x;
        let mut refused = 0;
        for character in text.chars() {
            // the face is copied out before anything is drawn: drawing borrows the frame mutably, so the font
            // cannot be held across it
            let (ink, rows, advance) = {
                let (face, ink) = self.glyph_here(character);
                (ink, face.rows, face.advance)
            };
            for (column, byte) in ink.iter().enumerate() {
                for row in 0..rows {
                    if byte & (1 << row) == 0 {
                        continue;
                    }
                    let px = pen + column as isize;
                    if px < 0 {
                        continue;
                    }
                    let (px, py) = (px as usize, y + row);
                    if self.inside(px, py) && self.get(px, py) {
                        refused += 1;
                        continue;
                    }
                    self.set(px, py, true);
                }
            }
            pen += advance as isize;
        }
        refused
    }

    /// The same text, in reverse: every pixel the glyphs would light is cleared instead.
    ///
    /// This is what a selected button's label is. A button is a filled box with its words inside it, so the one
    /// the pad is on is filled in and its words are holes rather than ink - the alternative, a border round a
    /// box that already has one, says nothing.
    pub fn text_unlit(&mut self, x: usize, y: usize, text: &str) {
        let mut pen = x;
        for character in text.chars() {
            // the face is copied out before anything is drawn: drawing borrows the frame mutably, so the font
            // cannot be held across it
            let (ink, rows, advance) = {
                let (face, ink) = self.glyph_here(character);
                (ink, face.rows, face.advance)
            };
            for (column, byte) in ink.iter().enumerate() {
                for row in 0..rows {
                    if byte & (1 << row) != 0 {
                        self.set(pen + column, y + row, false);
                    }
                }
            }
            pen += advance;
        }
    }

    /// Draw text on one of the five visible text rows.
    pub fn text_line(&mut self, line: usize, text: &str) {
        self.text(
            0,
            line.min(TEXT_ROWS.saturating_sub(1)) * font::LINE_HEIGHT,
            text,
        );
    }

    /// Whether every pixel of a rectangle is blank.
    ///
    /// This is the ink rule's question: an applet may draw text only where it will land on blank pixels, so
    /// that words never sit on top of a picture and become unreadable.
    pub fn is_blank(&self, x: usize, y: usize, width: usize, height: usize) -> bool {
        for row in y..y + height {
            for column in x..x + width {
                if self.get(column, row) {
                    return false;
                }
            }
        }
        true
    }

    /// Draw text, but only on blank pixels.
    ///
    /// Returns how many pixels were refused because ink was already there. That is the ink rule with teeth:
    /// text that lands on a picture is reported by the number of pixels it could not draw, rather than being
    /// drawn anyway and quietly unreadable.
    pub fn text_where_blank(&mut self, x: usize, y: usize, text: &str) -> usize {
        let mut refused = 0;
        let mut pen = x;
        for character in text.chars() {
            // the face is copied out before anything is drawn: drawing borrows the frame mutably, so the font
            // cannot be held across it
            let (ink, rows, advance) = {
                let (face, ink) = self.glyph_here(character);
                (ink, face.rows, face.advance)
            };
            for (column, byte) in ink.iter().enumerate() {
                for row in 0..rows {
                    if byte & (1 << row) != 0 {
                        let (px, py) = (pen + column, y + row);
                        if Self::offset(px, py).is_none() {
                            continue;
                        }
                        if self.get(px, py) {
                            refused += 1;
                        } else {
                            self.set(px, py, true);
                        }
                    }
                }
            }
            pen += advance;
        }
        refused
    }

    /// Copy the ink of another frame in, skipping any pixel that already has ink here.
    ///
    /// Returns how many pixels were refused, so a bar drawn over something is reported rather than laid on
    /// top of it.
    pub fn paste_where(&mut self, other: &Frame, x: usize, y: usize) -> usize {
        let mut refused = 0;
        for row in 0..HEIGHT {
            for column in 0..WIDTH {
                if !other.get(column, row) {
                    continue;
                }
                let (px, py) = (x + column, y + row);
                if Self::offset(px, py).is_none() {
                    continue;
                }
                if self.get(px, py) {
                    refused += 1;
                } else {
                    self.set(px, py, true);
                }
            }
        }
        refused
    }

    /// Draw a pointer of the given radius, from `(x, y)`, at a heading in degrees: zero is up, turning
    /// clockwise. Used by a compass or a waypoint arrow.
    pub fn arrow(&mut self, x: usize, y: usize, radius: usize, heading: f64) {
        let radians = heading.to_radians();
        let at = |along: f64| {
            (
                (x as f64 + along * radians.sin()).round().max(0.0) as usize,
                (y as f64 - along * radians.cos()).round().max(0.0) as usize,
            )
        };

        // the shaft: four points per pixel, so there are no gaps in the line
        let steps = (radius * 4).max(1);
        for step in 0..=steps {
            let (px, py) = at(radius as f64 * step as f64 / steps as f64);
            self.set(px, py, true);
        }

        // the head: two short arms angled off the tip
        let (tip_x, tip_y) = at(radius as f64);
        for turn in [-150.0_f64, 150.0] {
            let arm = radians + turn.to_radians();
            let length = radius.min(4);
            for along in 0..=length {
                let px = (tip_x as f64 + along as f64 * arm.sin()).round().max(0.0) as usize;
                let py = (tip_y as f64 - along as f64 * arm.cos()).round().max(0.0) as usize;
                self.set(px, py, true);
            }
        }
    }

    /// The whole image, as it goes on the wire.
    pub fn image(&self) -> &[u8] {
        &self.bits
    }

    /// The transfer to the device: report id, 31 header bytes, then the image.
    pub fn to_report(&self) -> [u8; REPORT_LEN] {
        let mut report = [0u8; REPORT_LEN];
        report[0] = REPORT_ID;
        report[1 + HEADER..].copy_from_slice(&self.bits);
        report
    }

    /// A frame made of the whole image, for comparing two frames.
    pub fn from_image(image: &[u8]) -> Option<Self> {
        if image.len() != BYTES {
            return None;
        }
        let mut frame = Self::new();
        frame.bits.copy_from_slice(image);
        Some(frame)
    }
}

impl std::fmt::Debug for Frame {
    /// Show it as it looks, so a test failure is readable.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(formatter)?;
        for y in 0..VISIBLE_HEIGHT {
            for x in 0..WIDTH {
                formatter.write_str(if self.get(x, y) { "#" } else { "." })?;
            }
            writeln!(formatter)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod clip_tests {
    use super::*;

    #[test]
    fn a_clipped_frame_keeps_its_ink_inside_the_rectangle() {
        // a running text is stopped at the width it was given rather than drawing across its neighbour
        let mut frame = Frame::new();
        frame.clipped(12, 0, 18, 8);
        frame.text(0, 0, "this is wider than the box it is drawn in");
        frame.unclipped();
        for x in 0..WIDTH {
            for y in 0..8 {
                if !frame.get(x, y) {
                    continue;
                }
                assert!(
                    (12..30).contains(&x),
                    "ink at ({x}, {y}) is outside the clip it was drawn in"
                );
            }
        }
        assert!(
            (12..30).any(|x| (0..8).any(|y| frame.get(x, y))),
            "the clip threw the text away altogether"
        );
        // and clearing it draws anywhere again
        let mut again = Frame::new();
        again.clipped(12, 0, 18, 8);
        again.unclipped();
        again.text(0, 0, "H");
        assert!(
            (0..WIDTH).any(|x| (0..8).any(|y| again.get(x, y))),
            "unclipped drawing is still clipped"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pixel_lands_where_the_device_expects_it() {
        let mut frame = Frame::new();
        frame.set(0, 0, true);
        assert_eq!(frame.image()[0], 0b0000_0001);
        frame.set(0, 7, true);
        assert_eq!(frame.image()[0], 0b1000_0001);
        // the ninth row is the second band, so column 0 of that band
        frame.set(0, 8, true);
        assert_eq!(frame.image()[WIDTH], 0b0000_0001);
        // and the last column of the first band is the 160th byte
        frame.set(159, 3, true);
        assert_eq!(frame.image()[159], 0b0000_1000);
    }

    #[test]
    fn the_image_is_the_size_the_descriptor_says() {
        let frame = Frame::new();
        assert_eq!(frame.image().len(), 960);
        let report = frame.to_report();
        assert_eq!(report.len(), 992, "report id + 31 header + 960 image");
        assert_eq!(report[0], 0x03);
        assert!(
            report[1..32].iter().all(|byte| *byte == 0),
            "the header is zeroed"
        );
    }

    #[test]
    fn out_of_range_coordinates_do_not_wrap_onto_the_line_below() {
        let mut frame = Frame::new();
        frame.set(WIDTH, 0, true);
        frame.set(0, HEIGHT, true);
        assert!(
            frame.image().iter().all(|byte| *byte == 0),
            "nothing should have been drawn"
        );
        assert!(!frame.get(WIDTH, 0));
    }

    #[test]
    fn text_lands_on_the_rows_it_is_given() {
        let mut frame = Frame::new();
        frame.text(0, 0, "A");
        assert!(
            frame.get(1, 1),
            "the first stroke of A should be ink near its top-left"
        );
        assert!(
            frame.is_blank(0, 8, WIDTH, 8),
            "the row below must stay blank"
        );

        let mut lines = Frame::new();
        lines.text_line(1, "A");
        assert!(
            lines.is_blank(0, 0, WIDTH, 8),
            "line 1 must not touch line 0"
        );
        assert!(
            !lines.is_blank(0, 8, WIDTH, 8),
            "line 1 should have ink in the second band"
        );
    }

    #[test]
    fn text_past_the_right_edge_is_clipped_not_wrapped() {
        let mut frame = Frame::new();
        frame.text(152, 0, "AAA");
        assert!(
            frame.is_blank(0, 8, WIDTH, 8),
            "nothing may appear on the next line"
        );
        assert!(
            !frame.is_blank(152, 0, 8, 8),
            "the first A should still be there"
        );
        // exactly 26 characters fit across, at six pixels each
        assert_eq!(TEXT_COLUMNS, 26);
        let mut full = Frame::new();
        full.text(0, 0, &"A".repeat(26));
        assert!(!full.is_blank(0, 0, WIDTH, 8));
        assert!(full.is_blank(0, 8, WIDTH, 8));
    }

    #[test]
    fn the_ink_rule_can_be_asked() {
        let mut frame = Frame::new();
        assert!(frame.is_blank(0, 0, WIDTH, VISIBLE_HEIGHT));
        frame.fill(10, 10, 20, 20, true);
        assert!(
            !frame.is_blank(5, 5, 40, 40),
            "the filled box is in the way"
        );
        assert!(frame.is_blank(0, 0, 5, 5), "the corner is still blank");
        frame.fill(10, 10, 20, 20, false);
        assert!(
            frame.is_blank(0, 0, WIDTH, VISIBLE_HEIGHT),
            "and it can be blank again"
        );
    }

    #[test]
    fn five_rows_of_text_fit_and_none_of_them_are_invisible() {
        assert_eq!(TEXT_ROWS, 5);
        let mut frame = Frame::new();
        for line in 0..TEXT_ROWS {
            frame.text_line(line, "row");
        }
        // every drawn row is inside the part of the panel a person can see
        for y in VISIBLE_HEIGHT..HEIGHT {
            for x in 0..WIDTH {
                assert!(
                    !frame.get(x, y),
                    "nothing should be drawn below the visible area"
                );
            }
        }
    }
}

#[cfg(test)]
mod shift_tests {
    use super::*;

    #[test]
    fn shifting_moves_the_ink_sideways_and_drops_what_leaves_the_panel() {
        let mut frame = Frame::new();
        frame.fill(10, 5, 4, 3, true);
        frame.shift(2);
        assert!(frame.get(12, 5), "ink moved right by two");
        assert!(!frame.get(10, 5), "and left where it was");
        assert!(!frame.get(8, 5));
        frame.shift(-2);
        assert!(frame.get(10, 5), "and back again");
        // off the edge is dropped, never wrapped round
        let mut edge = Frame::new();
        edge.fill(0, 0, 3, 1, true);
        edge.shift(-1);
        assert!(!edge.get(WIDTH - 1, 0), "nothing wrapped to the far side");
        assert!(edge.get(0, 0) || edge.get(1, 0));
        // and shifting by nothing is not a way to lose the picture
        let mut still = Frame::new();
        still.fill(4, 4, 2, 2, true);
        let before = still.clone();
        still.shift(0);
        assert_eq!(before, still);
    }
}
