//! The slide show's pure logic: where the show is (slide and build step,
//! or the end screen), typed slide numbers, which display gets which
//! window, and the presenter's timer. No GPUI types, so it's unit tested.

use std::time::{Duration, Instant};

/// Where the show is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pos {
    /// A slide (index into the presentation) and how many of its build
    /// steps have played (0 = as the slide first appears).
    Slide { index: usize, step: usize },
    /// The black "End of slide show" screen after the last slide.
    End,
}

/// The show's navigation over the deck. Hidden slides (`show="0"`) are
/// skipped by next / previous / first / last, but a typed number or the
/// slide grid can still go to them (as in PowerPoint).
#[derive(Clone, Debug)]
pub struct Nav {
    /// Per slide: hidden in the show.
    hidden: Vec<bool>,
    /// Per slide: how many click steps (builds) it has. All 0 until
    /// animations are played; [`Nav::advance`] / [`Nav::retreat`] already
    /// walk them.
    steps: Vec<usize>,
    pos: Pos,
}

impl Nav {
    /// A show over slides with these `hidden` flags, starting at `start`
    /// (or the first shown slide after it, else the last shown one before).
    pub fn new(hidden: Vec<bool>, start: usize) -> Self {
        let steps = vec![0; hidden.len()];
        let mut nav = Self {
            hidden,
            steps,
            pos: Pos::End,
        };
        let start = start.min(nav.len().saturating_sub(1));
        // Shift+F5 on a hidden slide shows it, as PowerPoint does.
        if start < nav.len() {
            nav.pos = Pos::Slide {
                index: start,
                step: 0,
            };
        }
        nav
    }

    pub fn len(&self) -> usize {
        self.hidden.len()
    }

    #[allow(dead_code)] // pairs with `len`
    pub fn is_empty(&self) -> bool {
        self.hidden.is_empty()
    }

    pub fn pos(&self) -> Pos {
        self.pos
    }

    /// The slide on screen (`None` on the end screen).
    pub fn slide(&self) -> Option<usize> {
        match self.pos {
            Pos::Slide { index, .. } => Some(index),
            Pos::End => None,
        }
    }

    /// The last slide the show was on (for the editor when it ends).
    pub fn last_slide(&self) -> usize {
        self.slide()
            .or_else(|| self.last_shown())
            .unwrap_or(0)
            .min(self.len().saturating_sub(1))
    }

    pub fn is_hidden(&self, index: usize) -> bool {
        self.hidden.get(index).copied().unwrap_or(false)
    }

    /// Set how many build steps `index` has (for animations, later).
    #[allow(dead_code)] // the hook for per-click builds
    pub fn set_steps(&mut self, index: usize, steps: usize) {
        if let Some(s) = self.steps.get_mut(index) {
            *s = steps;
        }
    }

    /// The deck changed while showing (an edit, an agent): new flags,
    /// keeping the position in range.
    pub fn update_slides(&mut self, hidden: Vec<bool>) {
        self.steps.resize(hidden.len(), 0);
        self.hidden = hidden;
        if let Pos::Slide { index, .. } = self.pos
            && index >= self.len()
        {
            self.pos = match self.last_shown() {
                Some(index) => Pos::Slide { index, step: 0 },
                None => Pos::End,
            };
        }
    }

    fn next_shown(&self, after: usize) -> Option<usize> {
        (after + 1..self.len()).find(|&i| !self.is_hidden(i))
    }

    fn prev_shown(&self, before: usize) -> Option<usize> {
        (0..before.min(self.len()))
            .rev()
            .find(|&i| !self.is_hidden(i))
    }

    pub fn first_shown(&self) -> Option<usize> {
        (0..self.len()).find(|&i| !self.is_hidden(i))
    }

    pub fn last_shown(&self) -> Option<usize> {
        (0..self.len()).rev().find(|&i| !self.is_hidden(i))
    }

    /// The slide after the current one (the presenter's "next" preview).
    pub fn upcoming(&self) -> Option<usize> {
        match self.pos {
            Pos::Slide { index, .. } => self.next_shown(index),
            Pos::End => None,
        }
    }

    /// Next build step, else the next shown slide, else the end screen.
    /// Returns whether the show is still running (`false`: the end screen
    /// was already up, so the click ends the show).
    pub fn advance(&mut self) -> bool {
        match self.pos {
            Pos::Slide { index, step } => {
                if step < self.steps.get(index).copied().unwrap_or(0) {
                    self.pos = Pos::Slide {
                        index,
                        step: step + 1,
                    };
                } else {
                    self.pos = match self.next_shown(index) {
                        Some(index) => Pos::Slide { index, step: 0 },
                        None => Pos::End,
                    };
                }
                true
            }
            Pos::End => false,
        }
    }

    /// Previous build step, else the previous shown slide fully built. On
    /// the first slide nothing changes.
    pub fn retreat(&mut self) {
        let full = |nav: &Self, index: usize| Pos::Slide {
            index,
            step: nav.steps.get(index).copied().unwrap_or(0),
        };
        self.pos = match self.pos {
            Pos::End => match self.last_shown() {
                Some(i) => full(self, i),
                None => Pos::End,
            },
            Pos::Slide { index, step } if step > 0 => Pos::Slide {
                index,
                step: step - 1,
            },
            Pos::Slide { index, step } => match self.prev_shown(index) {
                Some(i) => full(self, i),
                None => Pos::Slide { index, step },
            },
        };
    }

    pub fn first(&mut self) {
        if let Some(index) = self.first_shown() {
            self.pos = Pos::Slide { index, step: 0 };
        }
    }

    pub fn last(&mut self) {
        if let Some(index) = self.last_shown() {
            self.pos = Pos::Slide { index, step: 0 };
        }
    }

    /// Go to slide `index` (hidden ones too). Out of range: no change.
    pub fn go_to(&mut self, index: usize) -> bool {
        if index >= self.len() {
            return false;
        }
        self.pos = Pos::Slide { index, step: 0 };
        true
    }

    /// "Slide X of Y" counts: X among shown slides (a hidden slide on
    /// screen counts as its own place), Y the shown slides.
    pub fn ordinal(&self) -> (usize, usize) {
        let total = (0..self.len()).filter(|&i| !self.is_hidden(i)).count();
        match self.pos {
            Pos::Slide { index, .. } => {
                let before = (0..index).filter(|&i| !self.is_hidden(i)).count();
                (before + 1, total.max(before + 1))
            }
            Pos::End => (total, total),
        }
    }
}

/// Digits typed during the show; Enter goes to that slide number.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NumberEntry {
    digits: String,
}

impl NumberEntry {
    /// A typed key: digits are kept (up to 5), anything else is ignored.
    /// Returns whether the key was a digit.
    pub fn push(&mut self, key: &str) -> bool {
        let mut chars = key.chars();
        match (chars.next(), chars.next()) {
            (Some(c), None) if c.is_ascii_digit() => {
                if self.digits.len() < 5 {
                    self.digits.push(c);
                }
                true
            }
            _ => false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.digits.is_empty()
    }

    pub fn text(&self) -> &str {
        &self.digits
    }

    pub fn clear(&mut self) {
        self.digits.clear();
    }

    /// Enter: the 0-based slide index typed (1-based on screen), if it's
    /// within `len` slides. Clears the entry either way.
    pub fn take(&mut self, len: usize) -> Option<usize> {
        let n: usize = std::mem::take(&mut self.digits).parse().ok()?;
        (1..=len).contains(&n).then(|| n - 1)
    }
}

/// A display as the show sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen<Id> {
    pub id: Id,
    /// Top-left corner, to tell the displays apart in a stable order.
    pub origin: (f32, f32),
}

/// Which display shows the slides (the audience) and which one the
/// presenter view: with two or more displays the slides go on one that
/// isn't the presenter's (the editor's display, else the primary one);
/// with one display there's no presenter display.
pub fn choose_displays<Id: Copy + PartialEq>(
    screens: &[Screen<Id>],
    primary: Option<Id>,
    editor: Option<Id>,
) -> (Option<Id>, Option<Id>) {
    let known = |id: Option<Id>| id.filter(|id| screens.iter().any(|s| s.id == *id));
    if screens.len() < 2 {
        return (known(editor).or(screens.first().map(|s| s.id)), None);
    }
    let presenter = known(editor).or(known(primary)).unwrap_or(screens[0].id);
    let audience = screens
        .iter()
        .filter(|s| s.id != presenter)
        .min_by(|a, b| {
            a.origin
                .partial_cmp(&b.origin)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|s| s.id);
    (audience, Some(presenter))
}

/// The presenter's elapsed-time clock (pause / resume / reset).
#[derive(Clone, Copy, Debug)]
pub struct Stopwatch {
    started: Instant,
    /// Time counted before `started` (earlier runs before a pause).
    banked: Duration,
    paused: bool,
}

impl Stopwatch {
    pub fn new(now: Instant) -> Self {
        Self {
            started: now,
            banked: Duration::ZERO,
            paused: false,
        }
    }

    pub fn elapsed(&self, now: Instant) -> Duration {
        if self.paused {
            self.banked
        } else {
            self.banked + now.saturating_duration_since(self.started)
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn toggle_pause(&mut self, now: Instant) {
        if self.paused {
            self.started = now;
        } else {
            self.banked = self.elapsed(now);
        }
        self.paused = !self.paused;
    }

    pub fn reset(&mut self, now: Instant) {
        self.started = now;
        self.banked = Duration::ZERO;
    }
}

/// `m:ss`, or `h:mm:ss` from an hour.
pub fn format_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(index: usize) -> Pos {
        Pos::Slide { index, step: 0 }
    }

    #[std::prelude::v1::test]
    fn next_and_previous_skip_hidden_slides() {
        let mut nav = Nav::new(vec![false, true, false, true], 0);
        assert_eq!(nav.pos(), at(0));
        assert!(nav.advance());
        assert_eq!(nav.pos(), at(2));
        assert_eq!(nav.upcoming(), None);
        // Past the last shown slide: the end screen, then the click ends.
        assert!(nav.advance());
        assert_eq!(nav.pos(), Pos::End);
        assert!(!nav.advance());
        nav.retreat();
        assert_eq!(nav.pos(), at(2));
        nav.retreat();
        assert_eq!(nav.pos(), at(0));
        nav.retreat();
        assert_eq!(nav.pos(), at(0));
    }

    #[std::prelude::v1::test]
    fn first_last_and_jumps() {
        let mut nav = Nav::new(vec![true, false, false, true], 2);
        assert_eq!(nav.pos(), at(2));
        nav.first();
        assert_eq!(nav.pos(), at(1));
        nav.last();
        assert_eq!(nav.pos(), at(2));
        // A typed number reaches a hidden slide.
        assert!(nav.go_to(3));
        assert_eq!(nav.pos(), at(3));
        assert!(!nav.go_to(4));
        assert!(nav.advance());
        assert_eq!(nav.pos(), Pos::End);
        assert_eq!(nav.last_slide(), 2);
    }

    #[std::prelude::v1::test]
    fn starts_on_a_hidden_slide_when_asked() {
        let nav = Nav::new(vec![false, true, false], 1);
        assert_eq!(nav.pos(), at(1));
        assert_eq!(nav.upcoming(), Some(2));
        assert_eq!(Nav::new(vec![false, false], 9).pos(), at(1));
    }

    #[std::prelude::v1::test]
    fn build_steps_come_before_the_next_slide() {
        let mut nav = Nav::new(vec![false, false], 0);
        nav.set_steps(0, 2);
        nav.advance();
        assert_eq!(nav.pos(), Pos::Slide { index: 0, step: 1 });
        nav.advance();
        assert_eq!(nav.pos(), Pos::Slide { index: 0, step: 2 });
        nav.advance();
        assert_eq!(nav.pos(), at(1));
        // Back onto a slide: fully built, then step by step.
        nav.retreat();
        assert_eq!(nav.pos(), Pos::Slide { index: 0, step: 2 });
        nav.retreat();
        assert_eq!(nav.pos(), Pos::Slide { index: 0, step: 1 });
    }

    #[std::prelude::v1::test]
    fn ordinal_counts_shown_slides() {
        let mut nav = Nav::new(vec![false, true, false, false], 0);
        assert_eq!(nav.ordinal(), (1, 3));
        nav.advance();
        assert_eq!(nav.ordinal(), (2, 3));
        nav.go_to(1);
        assert_eq!(nav.ordinal(), (2, 3));
        nav.last();
        nav.advance();
        assert_eq!(nav.ordinal(), (3, 3));
    }

    #[std::prelude::v1::test]
    fn deck_shrinking_keeps_position_in_range() {
        let mut nav = Nav::new(vec![false; 5], 4);
        nav.update_slides(vec![false, false, true]);
        assert_eq!(nav.pos(), at(1));
        nav.update_slides(vec![true]);
        assert_eq!(nav.pos(), Pos::End);
        assert_eq!(nav.last_slide(), 0);
    }

    #[std::prelude::v1::test]
    fn all_hidden_deck_ends_at_once() {
        let mut nav = Nav::new(vec![true, true], 0);
        assert_eq!(nav.pos(), at(0));
        assert!(nav.advance());
        assert_eq!(nav.pos(), Pos::End);
        nav.retreat();
        assert_eq!(nav.pos(), Pos::End);
    }

    #[std::prelude::v1::test]
    fn typed_numbers() {
        let mut e = NumberEntry::default();
        assert!(e.push("1"));
        assert!(e.push("2"));
        assert!(!e.push("a"));
        assert!(!e.push("enter"));
        assert_eq!(e.text(), "12");
        assert_eq!(e.take(20), Some(11));
        assert!(e.is_empty());
        e.push("0");
        assert_eq!(e.take(20), None);
        e.push("2");
        e.push("1");
        assert_eq!(e.take(20), None);
        assert_eq!(e.take(20), None);
        for _ in 0..9 {
            e.push("9");
        }
        assert_eq!(e.text(), "99999");
    }

    #[std::prelude::v1::test]
    fn display_choice() {
        let one = [Screen {
            id: 1u32,
            origin: (0., 0.),
        }];
        assert_eq!(choose_displays(&one, None, Some(1)), (Some(1), None));
        assert_eq!(choose_displays(&one, None, Some(7)), (Some(1), None));
        assert_eq!(choose_displays::<u32>(&[], None, None), (None, None));
        let two = [
            Screen {
                id: 1u32,
                origin: (0., 0.),
            },
            Screen {
                id: 2,
                origin: (1920., 0.),
            },
        ];
        // The editor's display keeps the presenter view.
        assert_eq!(choose_displays(&two, Some(1), Some(2)), (Some(1), Some(2)));
        // Unknown editor display (Wayland before the first frame): primary.
        assert_eq!(choose_displays(&two, Some(2), None), (Some(1), Some(2)));
        assert_eq!(choose_displays(&two, None, None), (Some(2), Some(1)));
    }

    #[std::prelude::v1::test]
    fn stopwatch_pauses_and_resets() {
        let t0 = Instant::now();
        let s = |n| t0 + Duration::from_secs(n);
        let mut w = Stopwatch::new(t0);
        assert_eq!(w.elapsed(s(5)), Duration::from_secs(5));
        w.toggle_pause(s(5));
        assert!(w.is_paused());
        assert_eq!(w.elapsed(s(60)), Duration::from_secs(5));
        w.toggle_pause(s(60));
        assert_eq!(w.elapsed(s(70)), Duration::from_secs(15));
        w.reset(s(70));
        assert_eq!(w.elapsed(s(71)), Duration::from_secs(1));
        assert_eq!(format_elapsed(Duration::from_secs(65)), "1:05");
        assert_eq!(format_elapsed(Duration::from_secs(3725)), "1:02:05");
    }
}
