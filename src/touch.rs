//! Touch gestures as mouse input: a tap clicks, a long press right-clicks, a drag holds the left
//! button, and a two-finger vertical drag turns the wheel.

use std::collections::HashMap;

/// How long a finger must rest to right-click.
pub const LONG_PRESS_MS: u64 = 500;
/// How far (dp) a finger may wander and still tap.
pub const SLOP_DP: f64 = 8.0;
/// The two-finger travel (dp) of one wheel notch.
pub const WHEEL_STEP_DP: f64 = 48.0;

/// The mouse buttons gestures press.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

/// A mouse event a gesture makes, in surface pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Gesture {
    Move(f64, f64),
    Press(Button),
    Release(Button),
    /// Notches; `> 0` is up.
    Wheel(f32),
    /// The pointer left the view: nothing is hovered.
    Leave,
}

/// A finger's phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    /// No finger is down.
    Idle,
    /// One finger is down and has not decided between a tap, a drag and a long press.
    Pending {
        id: u64,
        start: (f64, f64),
        at: u64,
    },
    /// The finger `id` drags with the left button held.
    Dragging { id: u64 },
    /// A long press right-clicked; nothing more until every finger is up.
    Consumed,
    /// Fingers `a` and `b` scroll; `acc` is the travel (px) not yet turned into notches.
    TwoFinger {
        a: u64,
        b: u64,
        acc: f64,
        last_avg_y: f64,
    },
    /// A gesture ended; nothing more until every finger is up.
    Lifted,
}

/// The gesture state machine.
#[derive(Debug)]
pub struct Touch {
    /// Physical pixels per dp.
    scale: f64,
    state: State,
    /// The fingers down and their positions.
    down: HashMap<u64, (f64, f64)>,
}

impl Touch {
    pub fn new(scale: f64) -> Touch {
        Touch {
            scale,
            state: State::Idle,
            down: HashMap::new(),
        }
    }

    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale;
    }

    /// The gestures of one finger event at `(x, y)` surface pixels.
    pub fn event(&mut self, id: u64, phase: Phase, x: f64, y: f64, now_ms: u64) -> Vec<Gesture> {
        match phase {
            Phase::Started | Phase::Moved => {
                if phase == Phase::Moved && !self.down.contains_key(&id) {
                    // A finger that was down before a reset.
                    return Vec::new();
                }
                self.down.insert(id, (x, y));
            }
            Phase::Ended | Phase::Cancelled => {
                if self.down.remove(&id).is_none() {
                    return Vec::new();
                }
            }
        }
        let mut out = Vec::new();
        match (self.state, phase) {
            (State::Idle, Phase::Started) => {
                out.push(Gesture::Move(x, y));
                self.state = State::Pending {
                    id,
                    start: (x, y),
                    at: now_ms,
                };
            }
            (State::Idle, _) => {}
            (State::Pending { id: first, .. }, Phase::Started) => {
                let avg = self.average_y(first, id);
                self.state = State::TwoFinger {
                    a: first,
                    b: id,
                    acc: 0.0,
                    last_avg_y: avg,
                };
            }
            (State::Pending { id: own, start, .. }, Phase::Moved) if own == id => {
                let (dx, dy) = (x - start.0, y - start.1);
                let slop = SLOP_DP * self.scale;
                if dx * dx + dy * dy > slop * slop {
                    out.extend([Gesture::Press(Button::Left), Gesture::Move(x, y)]);
                    self.state = State::Dragging { id };
                }
            }
            (State::Pending { id: own, .. }, Phase::Ended) if own == id => {
                out.extend([
                    Gesture::Press(Button::Left),
                    Gesture::Release(Button::Left),
                    Gesture::Leave,
                ]);
                self.state = State::Idle;
            }
            (State::Pending { id: own, .. }, Phase::Cancelled) if own == id => {
                out.push(Gesture::Leave);
                self.state = State::Idle;
            }
            (State::Pending { .. }, _) => {}
            (State::Dragging { id: own }, Phase::Moved) if own == id => {
                out.push(Gesture::Move(x, y));
            }
            (State::Dragging { id: own }, Phase::Ended | Phase::Cancelled) if own == id => {
                out.push(Gesture::Release(Button::Left));
                // Other fingers still down: the pointer leaves once they are up too.
                self.state = State::Lifted;
            }
            (State::Dragging { .. }, _) => {}
            (
                State::TwoFinger {
                    a,
                    b,
                    acc,
                    last_avg_y,
                },
                Phase::Moved,
            ) if id == a || id == b => {
                let avg = self.average_y(a, b);
                let mut acc = acc + (avg - last_avg_y);
                let step = WHEEL_STEP_DP * self.scale;
                if step > 0.0 {
                    while acc >= step {
                        acc -= step;
                        out.push(Gesture::Wheel(1.0));
                    }
                    while acc <= -step {
                        acc += step;
                        out.push(Gesture::Wheel(-1.0));
                    }
                }
                self.state = State::TwoFinger {
                    a,
                    b,
                    acc,
                    last_avg_y: avg,
                };
            }
            (State::TwoFinger { .. }, Phase::Ended | Phase::Cancelled) => {
                self.state = State::Lifted;
            }
            (State::TwoFinger { .. } | State::Consumed | State::Lifted, _) => {}
        }
        if matches!(self.state, State::Consumed | State::Lifted) && self.down.is_empty() {
            out.push(Gesture::Leave);
            self.state = State::Idle;
        }
        out
    }

    /// The gestures time alone makes: a finger resting long enough right-clicks.
    pub fn tick(&mut self, now_ms: u64) -> Vec<Gesture> {
        match self.state {
            State::Pending { at, .. } if now_ms.saturating_sub(at) >= LONG_PRESS_MS => {
                self.state = State::Consumed;
                vec![
                    Gesture::Press(Button::Right),
                    Gesture::Release(Button::Right),
                ]
            }
            _ => Vec::new(),
        }
    }

    /// Forgets every finger (focus lost, app suspended), releasing what a gesture held.
    pub fn reset(&mut self) -> Vec<Gesture> {
        let mut out = Vec::new();
        if let State::Dragging { .. } = self.state {
            out.push(Gesture::Release(Button::Left));
        }
        if self.state != State::Idle {
            out.push(Gesture::Leave);
        }
        self.state = State::Idle;
        self.down.clear();
        out
    }

    /// The average y of two fingers down; a missing one counts as the other.
    fn average_y(&self, a: u64, b: u64) -> f64 {
        let y = |id| self.down.get(&id).map(|p| p.1);
        match (y(a), y(b)) {
            (Some(ya), Some(yb)) => (ya + yb) / 2.0,
            (Some(y), None) | (None, Some(y)) => y,
            (None, None) => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )]

    use super::*;
    use Gesture::*;
    use Phase::*;

    #[test]
    fn tap_clicks_and_leaves() {
        let mut t = Touch::new(2.0);
        assert_eq!(t.event(1, Started, 10.0, 20.0, 0), [Move(10.0, 20.0)]);
        assert_eq!(
            t.event(1, Ended, 10.0, 20.0, 100),
            [Press(Button::Left), Release(Button::Left), Leave]
        );
        // Idle again: the next finger starts a new gesture.
        assert_eq!(t.event(2, Started, 5.0, 5.0, 200), [Move(5.0, 5.0)]);
    }

    #[test]
    fn jitter_within_slop_still_taps() {
        let mut t = Touch::new(2.0); // slop 16 px
        t.event(1, Started, 100.0, 100.0, 0);
        assert!(t.event(1, Moved, 110.0, 108.0, 50).is_empty());
        assert_eq!(
            t.event(1, Ended, 110.0, 108.0, 80),
            [Press(Button::Left), Release(Button::Left), Leave]
        );
    }

    #[test]
    fn long_press_right_clicks_once() {
        let mut t = Touch::new(1.0);
        t.event(1, Started, 0.0, 0.0, 1000);
        assert!(t.tick(1499).is_empty());
        assert_eq!(
            t.tick(1500),
            [Press(Button::Right), Release(Button::Right)]
        );
        assert!(t.tick(3000).is_empty());
        // Moving and lifting after the right-click do nothing until the finger is up.
        assert!(t.event(1, Moved, 100.0, 100.0, 3100).is_empty());
        assert_eq!(t.event(1, Ended, 100.0, 100.0, 3200), [Leave]);
    }

    #[test]
    fn drag_holds_the_left_button() {
        let mut t = Touch::new(1.0); // slop 8 px
        t.event(1, Started, 0.0, 0.0, 0);
        assert_eq!(
            t.event(1, Moved, 9.0, 0.0, 10),
            [Press(Button::Left), Move(9.0, 0.0)]
        );
        assert_eq!(t.event(1, Moved, 30.0, 4.0, 20), [Move(30.0, 4.0)]);
        // No long press once dragging.
        assert!(t.tick(5000).is_empty());
        // A second finger is ignored while dragging.
        assert!(t.event(2, Started, 50.0, 50.0, 30).is_empty());
        assert!(t.event(2, Ended, 50.0, 50.0, 40).is_empty());
        assert_eq!(
            t.event(1, Ended, 30.0, 4.0, 50),
            [Release(Button::Left), Leave]
        );
    }

    #[test]
    fn drag_ending_with_another_finger_down_leaves_when_it_lifts() {
        let mut t = Touch::new(1.0);
        t.event(1, Started, 0.0, 0.0, 0);
        t.event(1, Moved, 20.0, 0.0, 10);
        t.event(2, Started, 50.0, 50.0, 20);
        assert_eq!(t.event(1, Ended, 20.0, 0.0, 30), [Release(Button::Left)]);
        assert_eq!(t.event(2, Ended, 50.0, 50.0, 40), [Leave]);
    }

    #[test]
    fn cancel_while_dragging_releases() {
        let mut t = Touch::new(1.0);
        t.event(1, Started, 0.0, 0.0, 0);
        t.event(1, Moved, 20.0, 0.0, 10);
        assert_eq!(
            t.event(1, Cancelled, 20.0, 0.0, 20),
            [Release(Button::Left), Leave]
        );
    }

    #[test]
    fn cancel_while_pending_only_leaves() {
        let mut t = Touch::new(1.0);
        t.event(1, Started, 0.0, 0.0, 0);
        assert_eq!(t.event(1, Cancelled, 0.0, 0.0, 10), [Leave]);
    }

    #[test]
    fn two_fingers_turn_the_wheel() {
        let mut t = Touch::new(1.0); // step 48 px
        t.event(1, Started, 100.0, 100.0, 0);
        assert!(t.event(2, Started, 200.0, 100.0, 10).is_empty());
        // Both fingers move down 120 px: 2.5 steps up, a notch per 48 px of the average.
        assert_eq!(t.event(1, Moved, 100.0, 220.0, 20), [Wheel(1.0)]); // average +60
        assert_eq!(t.event(2, Moved, 200.0, 220.0, 30), [Wheel(1.0)]); // average +120
        // Back up 72 px: the 24 px left over minus 72 is one notch down.
        t.event(1, Moved, 100.0, 148.0, 40);
        assert_eq!(t.event(2, Moved, 200.0, 148.0, 50), [Wheel(-1.0)]);
        // No long press while scrolling.
        assert!(t.tick(5000).is_empty());
        assert!(t.event(1, Ended, 100.0, 148.0, 60).is_empty());
        assert!(t.event(2, Moved, 200.0, 400.0, 70).is_empty());
        assert_eq!(t.event(2, Ended, 200.0, 400.0, 80), [Leave]);
    }

    #[test]
    fn reset_releases_and_forgets() {
        let mut t = Touch::new(1.0);
        assert!(t.reset().is_empty());
        t.event(1, Started, 0.0, 0.0, 0);
        t.event(1, Moved, 20.0, 0.0, 10);
        assert_eq!(t.reset(), [Release(Button::Left), Leave]);
        // The finger that was down is forgotten.
        assert!(t.event(1, Moved, 30.0, 0.0, 20).is_empty());
        assert!(t.event(1, Ended, 30.0, 0.0, 30).is_empty());
        t.event(2, Started, 0.0, 0.0, 40);
        assert_eq!(t.reset(), [Leave]);
    }
}
