//! The cycle queue: the ordered list of colours, and the position in it.
//!
//! Here rather than beside either front end because both play the same
//! sequence and must agree on every detail of it -- where the ring goes when
//! a step is removed, which step a repeated colour rings, what wrapping does
//! at each end. None of it needs a display, and all of it is tested below.

use crate::Rgba;

/// Colours a fresh queue starts with.
///
/// White and black drive every subpixel fully on and fully off; red, green
/// and blue light one at a time; cyan, magenta and yellow light them in
/// pairs, which is where a single dead subpixel stops hiding behind its
/// neighbours.
pub const DEFAULT: &[Rgba] = &[
    Rgba::new(255, 255, 255),
    Rgba::new(0, 0, 0),
    Rgba::new(255, 0, 0),
    Rgba::new(0, 255, 0),
    Rgba::new(0, 0, 255),
    Rgba::new(0, 255, 255),
    Rgba::new(255, 0, 255),
    Rgba::new(255, 255, 0),
];

/// Upper bound on the queue. The strip that edits it is one row and the
/// window has a width; past this it would either wrap or push the card out
/// of shape.
pub const MAX_LEN: usize = 16;

/// The list of colours and the position in it, with no widget attached.
///
/// Split out from the widget because this is the part with the awkward
/// arithmetic -- removing a step shifts every later one down, a queue may hold
/// the same colour twice, and the ring is chosen differently depending on
/// whether the timer or the user put it there -- and none of it needs a
/// display to test. (`dedup_idents` in main_window.rs is split out for the
/// same reason.)
#[derive(Debug, Clone, PartialEq)]
pub struct Steps {
    colors: Vec<Rgba>,
    /// Step the ring is on.
    index: Option<usize>,
    /// Whether the timer put it there.
    ///
    /// Both modes ring a step, but they choose it differently: playback knows
    /// the index it just moved to, while everything else only knows the colour
    /// now on screen and has to look it up. Keeping the flag is what makes a
    /// queue holding the same colour twice ring the step actually being played
    /// rather than the first one that matches.
    playing: bool,
}

impl Default for Steps {
    fn default() -> Self {
        Self { colors: DEFAULT.to_vec(), index: None, playing: false }
    }
}

impl Steps {
    pub fn colors(&self) -> &[Rgba] { &self.colors }

    pub fn len(&self) -> usize { self.colors.len() }

    pub fn is_empty(&self) -> bool { self.colors.is_empty() }

    /// Whether another step would fit; what makes "+" insensitive.
    pub fn is_full(&self) -> bool { self.len() >= MAX_LEN }

    pub fn index(&self) -> Option<usize> { self.index }

    /// Replace the list.
    ///
    /// A position into the old list says nothing about the new one, and
    /// keeping it left the ring on whichever step happened to inherit the
    /// index -- at startup, on the default list's step 0 rather than on the
    /// colour actually showing.
    pub fn set_colors(&mut self, colors: &[Rgba]) {
        self.colors = colors.iter().take(MAX_LEN).copied().collect();
        self.index = None;
    }

    /// Append a colour. False when the queue is full -- the button that gets
    /// here is insensitive by then, and a dialog for "sixteen is enough"
    /// would be worse than the limit.
    pub fn push(&mut self, rgba: Rgba) -> bool {
        if self.is_full() {
            return false;
        }
        self.colors.push(rgba);
        true
    }

    /// Drop a step.
    ///
    /// The ring follows the same *step*, not the same position: dropping an
    /// earlier step shifts every later one down by one. Dropping the ringed
    /// step clears the ring, and the caller decides whether some other step
    /// holds the colour still on screen.
    pub fn remove(&mut self, index: usize) -> bool {
        if index >= self.colors.len() {
            return false;
        }
        self.colors.remove(index);
        self.index = match self.index {
            Some(i) if i == index => None,
            Some(i) if i > index  => Some(i - 1),
            other                 => other,
        };
        true
    }

    /// Replace one step's colour, and ring it. False when the index is out of
    /// range, or the colour is the one already there.
    pub fn set_color_at(&mut self, index: usize, rgba: Rgba) -> bool {
        match self.colors.get_mut(index) {
            Some(slot) if *slot != rgba => *slot = rgba,
            _ => return false,
        }
        self.index = Some(index);
        true
    }

    pub fn position_of(&self, rgba: Rgba) -> Option<usize> {
        self.colors.iter().position(|c| *c == rgba)
    }

    /// Ring one step by position, whatever else holds its colour.
    pub fn select(&mut self, index: usize) {
        if index < self.colors.len() {
            self.index = Some(index);
        }
    }

    /// Begin playing, from the step showing `current` if the queue holds it
    /// and from the top otherwise -- so hitting Cycle while black is up
    /// carries on from black rather than jumping.
    pub fn start(&mut self, current: Rgba) -> Option<Rgba> {
        let index = self.position_of(current).unwrap_or(0);
        self.playing = true;
        self.seek(index)
    }

    /// The timer's move to the next step.
    pub fn advance(&mut self) -> Option<Rgba> {
        let next = self.neighbour(true)?;
        self.playing = true;
        self.seek(next)
    }

    /// A step taken by hand, from the overlay or the control window.
    ///
    /// Unlike `advance()` this does not claim the ring for the timer: stepping
    /// with the timer stopped has to leave the chips following the colour on
    /// screen, which is what `highlight()` does while `playing` is false.
    pub fn step(&mut self, forward: bool) -> Option<Rgba> {
        let next = self.neighbour(forward)?;
        self.seek(next)
    }

    /// Stop playing. The ring goes back to following whatever colour is on
    /// screen, which the caller sets with `highlight()`.
    pub fn stop(&mut self) {
        self.playing = false;
        self.index = None;
    }

    /// Ring the step holding `rgba`, or nothing if no step does.
    ///
    /// Ignored while playing: the timer's position is the authority then.
    ///
    /// A step that was picked out explicitly -- clicked, stepped to, or just
    /// edited -- keeps the ring as long as it still holds this colour. A queue
    /// may hold the same colour twice, and clicking the second red then
    /// resolving the ring by colour would snap it back to the first one.
    pub fn highlight(&mut self, rgba: Rgba) {
        if self.playing {
            return;
        }
        if self
            .index
            .is_some_and(|i| self.colors.get(i).is_some_and(|c| *c == rgba))
        {
            return;
        }
        self.index = self.position_of(rgba);
    }

    /// The step either side of the ring, wrapping at both ends. With no ring
    /// yet, the end the move came from.
    fn neighbour(&self, forward: bool) -> Option<usize> {
        let len = self.colors.len();
        if len == 0 {
            return None;
        }
        Some(match (self.index, forward) {
            (None, true)     => 0,
            (None, false)    => len - 1,
            (Some(i), true)  => (i + 1) % len,
            // Add len before subtracting: 0 - 1 wraps a usize.
            (Some(i), false) => (i + len - 1) % len,
        })
    }

    fn seek(&mut self, index: usize) -> Option<Rgba> {
        let rgba = self.colors.get(index).copied()?;
        self.index = Some(index);
        Some(rgba)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CYAN: Rgba = Rgba::new(0, 255, 255);

    fn steps(colors: &[Rgba]) -> Steps {
        let mut s = Steps::default();
        s.set_colors(colors);
        s
    }

    #[test]
    fn a_fresh_queue_is_the_default_sequence_with_no_ring() {
        let s = Steps::default();
        assert_eq!(s.colors(), DEFAULT);
        assert_eq!(s.index(), None);
    }

    #[test]
    fn the_queue_stops_at_its_cap() {
        let mut s = steps(&[Rgba::WHITE; MAX_LEN]);
        assert!(s.is_full());
        assert!(!s.push(Rgba::new(255, 0, 0)), "a full queue must refuse another step");
        assert_eq!(s.len(), MAX_LEN);

        // And a longer list handed in is truncated rather than rejected.
        s.set_colors(&[Rgba::BLACK; MAX_LEN + 4]);
        assert_eq!(s.len(), MAX_LEN);
    }

    #[test]
    fn removing_a_step_moves_the_ring_with_the_step_it_was_on() {
        let mut s = steps(&[Rgba::WHITE, Rgba::BLACK, Rgba::new(255, 0, 0)]);

        // The ring is on the last step; dropping an earlier one shifts it down.
        s.select(2);
        assert!(s.remove(0));
        assert_eq!(s.index(), Some(1));

        // Dropping the ringed step itself clears the ring.
        s.select(1);
        assert!(s.remove(1));
        assert_eq!(s.index(), None);

        // Out of range changes nothing.
        assert!(!s.remove(9));
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn replacing_a_list_clears_the_position_into_the_old_one() {
        let mut s = steps(&[Rgba::WHITE, Rgba::BLACK]);
        s.select(1);
        s.set_colors(&[Rgba::new(255, 0, 0), Rgba::new(0, 255, 0)]);
        assert_eq!(s.index(), None, "a position into the old list means nothing here");
    }

    #[test]
    fn play_carries_on_from_the_colour_already_showing() {
        let mut s = steps(&[Rgba::WHITE, Rgba::BLACK, Rgba::new(255, 0, 0)]);
        assert_eq!(s.start(Rgba::BLACK), Some(Rgba::BLACK));
        assert_eq!(s.index(), Some(1));

        // A colour no step holds starts from the top.
        let mut s = steps(&[Rgba::WHITE, Rgba::BLACK]);
        assert_eq!(s.start(CYAN), Some(Rgba::WHITE));
        assert_eq!(s.index(), Some(0));
    }

    #[test]
    fn stepping_wraps_in_both_directions() {
        let mut s = steps(&[Rgba::WHITE, Rgba::BLACK, Rgba::new(255, 0, 0)]);

        // With no ring yet, each direction starts from the end it came from.
        assert_eq!(s.step(true), Some(Rgba::WHITE));
        assert_eq!(s.step(true), Some(Rgba::BLACK));
        assert_eq!(s.step(true), Some(Rgba::new(255, 0, 0)));
        assert_eq!(s.step(true), Some(Rgba::WHITE), "forward wraps at the end");

        // Backwards off step 0 must wrap rather than underflow the usize.
        assert_eq!(s.step(false), Some(Rgba::new(255, 0, 0)));
        assert_eq!(s.index(), Some(2));

        let mut s = steps(&[Rgba::WHITE, Rgba::BLACK]);
        assert_eq!(s.step(false), Some(Rgba::BLACK), "backward starts at the end");
    }

    #[test]
    fn an_empty_queue_has_nowhere_to_step() {
        let mut s = steps(&[]);
        assert_eq!(s.step(true), None);
        assert_eq!(s.step(false), None);
        assert_eq!(s.advance(), None);
        assert_eq!(s.start(Rgba::WHITE), None);
    }

    #[test]
    fn the_timer_owns_the_ring_but_a_manual_step_does_not() {
        let list = [Rgba::WHITE, Rgba::BLACK, Rgba::new(255, 0, 0)];

        // advance() is the timer's move: highlight() must not fight it.
        let mut s = steps(&list);
        s.advance();
        s.highlight(Rgba::new(255, 0, 0));
        assert_eq!(s.index(), Some(0), "the timer's position is the authority");

        // step() is the user's move, so the ring still follows the colour.
        let mut s = steps(&list);
        s.step(true);
        s.highlight(Rgba::new(255, 0, 0));
        assert_eq!(s.index(), Some(2));
    }

    #[test]
    fn a_duplicated_colour_keeps_the_ring_on_the_step_chosen() {
        // Two reds: picking the second one and then being told "red is
        // showing" must not snap the ring back to the first.
        let mut s = steps(&[Rgba::new(255, 0, 0), Rgba::WHITE, Rgba::new(255, 0, 0)]);
        s.select(2);
        s.highlight(Rgba::new(255, 0, 0));
        assert_eq!(s.index(), Some(2));

        // A colour no step holds clears it.
        s.highlight(CYAN);
        assert_eq!(s.index(), None);
    }

    #[test]
    fn editing_a_step_rings_it_and_rejects_a_no_op() {
        let mut s = steps(&[Rgba::WHITE, Rgba::BLACK]);
        assert!(s.set_color_at(1, Rgba::new(255, 0, 0)));
        assert_eq!(s.colors()[1], Rgba::new(255, 0, 0));
        assert_eq!(s.index(), Some(1));

        assert!(!s.set_color_at(1, Rgba::new(255, 0, 0)), "the same colour is not a change");
        assert!(!s.set_color_at(7, Rgba::new(0, 0, 255)), "out of range is not a change");
    }

    #[test]
    fn a_colour_written_out_and_read_back_still_finds_its_step() {
        // The queue is saved as hex and restored from it, so a step has to
        // still be recognisable after the round trip.
        let mut s = steps(&[Rgba::WHITE, Rgba::new(128, 64, 191)]);
        let parsed = Rgba::parse_hex(&s.colors()[1].hex()).unwrap();
        assert_eq!(s.position_of(parsed), Some(1));
        s.highlight(parsed);
        assert_eq!(s.index(), Some(1));
    }
}
