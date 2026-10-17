//! Fingers on the page: tablets in the browser (Android tablets, iPads) and touch screens on
//! Windows and Linux.
//!
//! eframe and egui-winit report the first finger twice: as [`egui::Event::Touch`] and as the
//! primary mouse button. Left as the mouse, a finger drawn across the page selects text, so the
//! document can't be scrolled by hand. [`TouchGestures`] reads the touches that start on the page
//! instead and gives the canvas what a tablet word processor does:
//! - a swipe scrolls the page, and a flick coasts on ([`TouchGestures::scroll`]); in Read Mode a
//!   swipe turns the page ([`TouchGestures::take_swipe`]);
//! - a tap is a click, so two quick taps are a double-click (a word) and three a triple-click;
//! - touch and hold, then drag: the mouse drags from where the finger went down (select text, move
//!   a picture or a shape);
//! - touch and hold, then lift: a right-click (the context menu);
//! - two fingers pinch to zoom (the canvas reads egui's multi-touch) and scroll together.
//!
//! Touches that start anywhere else (ribbon, panes, scroll bars, menus, dialogs), and fingers on
//! the page while a Draw tab pen, eraser or lasso is on, stay the mouse.

use egui::{Event, PointerButton, Pos2, Rect, TouchId, TouchPhase, Vec2};

use crate::scroll::Momentum;

/// How far (points) a finger may wander before a tap or a hold becomes a swipe or a drag
/// (Android's touch slop is 8 dp).
const SLOP: f32 = 8.0;
/// Seconds a finger rests on the page before the touch is a hold rather than a tap.
const HOLD: f64 = 0.5;
/// A swipe in Read Mode turns the page once it travels this far (points).
const PAGE_SWIPE: f32 = 48.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Gesture {
    /// No finger down.
    #[default]
    Idle,
    /// Fingers that went down off the page (or to draw): their pointer events stay the mouse
    /// until every finger lifts.
    Mouse,
    /// One finger down on the page at `at` since input time `since`: not yet a tap, swipe or hold.
    Down { at: Pos2, since: Option<f64> },
    /// The page follows the fingers.
    Pan,
    /// Held, then moved: the primary button is down and follows `finger`.
    Drag { finger: TouchId },
}

/// Turns touches on the page into scrolling and clicks (see the module docs).
#[derive(Clone, Debug)]
pub struct TouchGestures {
    gesture: Gesture,
    /// Fingers down, by touch id, at their latest position.
    fingers: Vec<(TouchId, Pos2)>,
    /// Where touches act on the page: the canvas's viewport last frame, `None` when the page
    /// wasn't drawn or a dialog or menu was open. `page_next` collects this frame's.
    page: Option<Rect>,
    page_next: Option<Rect>,
    /// Scroll from the fingers since the canvas last asked (content movement, like egui's wheel
    /// deltas: positive moves the page down/right).
    pan: Vec2,
    /// The current swipe's whole travel, and a finished swipe's until Read Mode takes it.
    travel: Vec2,
    swipe: Option<Vec2>,
    /// The fingers lifted after a swipe: a flick coasts on.
    lifted: bool,
    momentum: Momentum,
    /// Events for the next frame: a click's release lands a frame after its press, as a mouse's.
    later: Vec<Event>,
    /// The keys held, for the clicks a finger makes (Shift+tap extends the selection).
    modifiers: egui::Modifiers,
    /// The primary button is down for a pointer of its own: a mouse, or a pen that the browser
    /// also reports as a touch. A touch that starts meanwhile is its shadow and stays the mouse.
    pointer_down: bool,
}

impl Default for TouchGestures {
    fn default() -> Self {
        Self {
            gesture: Gesture::Idle,
            fingers: Vec::new(),
            page: None,
            page_next: None,
            pan: Vec2::ZERO,
            travel: Vec2::ZERO,
            swipe: None,
            lifted: false,
            // A touch screen reports when the fingers lift, so a resting finger never coasts.
            momentum: Momentum::with_lift(),
            later: Vec::new(),
            modifiers: egui::Modifiers::NONE,
            pointer_down: false,
        }
    }
}

/// Most fingers tracked at once; more are ignored rather than grown without bound.
const MAX_FINGERS: usize = 10;

impl TouchGestures {
    /// Rewrite this frame's input before egui reads it (eframe's `raw_input_hook`). `draw`: a
    /// Draw tab pen, eraser or lasso is on, so a finger on the page draws as the mouse does.
    pub fn hook(&mut self, raw: &mut egui::RawInput, draw: bool) {
        for e in &raw.events {
            if let Event::ModifiersChanged(m) = e {
                self.modifiers = *m;
            }
        }
        let later = std::mem::take(&mut self.later);
        let touches = raw.events.iter().any(|e| matches!(e, Event::Touch { .. }));
        if later.is_empty() && !touches && self.gesture == Gesture::Idle {
            self.track_pointer(&raw.events);
            return;
        }
        let mut ours = self.is_ours();
        let mut made = Vec::new();
        for e in &raw.events {
            if let Event::Touch { id, phase, pos, .. } = *e {
                self.touch(id, phase, pos, raw.time, draw, &mut made);
                ours |= self.is_ours();
            }
        }
        if ours {
            // The mouse events made up for the first finger: the gesture speaks for it instead.
            raw.events
                .retain(|e| !matches!(e, Event::PointerButton { button: PointerButton::Primary, .. } | Event::PointerMoved(_) | Event::PointerGone));
        } else {
            self.track_pointer(&raw.events);
        }
        raw.events.splice(0..0, later);
        raw.events.extend(made);
    }

    /// Follow the primary button through events that reach egui as they are.
    fn track_pointer(&mut self, events: &[Event]) {
        for e in events {
            if let Event::PointerButton { button: PointerButton::Primary, pressed, .. } = e {
                self.pointer_down = *pressed;
            }
        }
    }

    fn is_ours(&self) -> bool {
        !matches!(self.gesture, Gesture::Idle | Gesture::Mouse)
    }

    fn touch(&mut self, id: TouchId, phase: TouchPhase, pos: Pos2, now: Option<f64>, draw: bool, made: &mut Vec<Event>) {
        // A lift is honoured wherever it says it happened; a finger can't go down or move to nowhere.
        if !pos.is_finite() && matches!(phase, TouchPhase::Start | TouchPhase::Move) {
            return;
        }
        let modifiers = self.modifiers;
        let button = |pos, button, pressed| Event::PointerButton { pos, button, pressed, modifiers };
        match phase {
            TouchPhase::Start => {
                let first = self.fingers.is_empty();
                self.fingers.retain(|(f, _)| *f != id);
                if self.fingers.len() < MAX_FINGERS {
                    self.fingers.push((id, pos));
                }
                if first {
                    // A finger on the glass stops a coasting page.
                    self.momentum.cancel();
                    self.travel = Vec2::ZERO;
                    let on_page = !draw && !self.pointer_down && self.page.is_some_and(|r| r.contains(pos));
                    self.gesture = if on_page { Gesture::Down { at: pos, since: now } } else { Gesture::Mouse };
                } else if matches!(self.gesture, Gesture::Down { .. }) {
                    // A second finger: pinch and scroll, never a click.
                    self.gesture = Gesture::Pan;
                }
            }
            TouchPhase::Move => {
                let n = self.fingers.len().max(1) as f32;
                let Some(slot) = self.fingers.iter_mut().find(|(f, _)| *f == id) else { return };
                let moved = pos - slot.1;
                slot.1 = pos;
                match self.gesture {
                    Gesture::Down { at, since } if (pos - at).length() > SLOP => {
                        if held(since, now) {
                            // The button goes down where the finger did; the move follows a
                            // frame later, as a mouse's would, so the drag starts there.
                            self.gesture = Gesture::Drag { finger: id };
                            made.extend([Event::PointerMoved(at), button(at, PointerButton::Primary, true)]);
                            self.later.push(Event::PointerMoved(pos));
                        } else {
                            // The page follows the finger from where it went down.
                            self.gesture = Gesture::Pan;
                            self.scroll_by(pos - at);
                        }
                    }
                    // With several fingers the page follows their centre.
                    Gesture::Pan => self.scroll_by(moved / n),
                    Gesture::Drag { finger } if finger == id => made.push(Event::PointerMoved(pos)),
                    _ => {}
                }
            }
            TouchPhase::End | TouchPhase::Cancel => {
                let Some(i) = self.fingers.iter().position(|(f, _)| *f == id) else { return };
                let (_, last) = self.fingers.remove(i);
                match self.gesture {
                    Gesture::Down { at, since } => {
                        if phase == TouchPhase::End {
                            let which = if held(since, now) { PointerButton::Secondary } else { PointerButton::Primary };
                            made.extend([Event::PointerMoved(at), button(at, which, true)]);
                            self.later.extend([button(at, which, false), Event::PointerGone]);
                        }
                        self.gesture = Gesture::Idle;
                    }
                    Gesture::Drag { finger } if finger == id => {
                        // Cancelled or not, the button comes up where the finger was last.
                        made.extend([button(last, PointerButton::Primary, false), Event::PointerGone]);
                        self.gesture = if self.fingers.is_empty() { Gesture::Idle } else { Gesture::Pan };
                    }
                    Gesture::Pan if self.fingers.is_empty() => {
                        self.lifted = true;
                        self.swipe = Some(self.travel);
                        self.gesture = Gesture::Idle;
                    }
                    _ if self.fingers.is_empty() => self.gesture = Gesture::Idle,
                    _ => {}
                }
            }
        }
    }

    fn scroll_by(&mut self, d: Vec2) {
        if d.is_finite() {
            self.pan += d;
            self.travel += d;
        }
    }

    /// Where touches act on the page this frame (the canvas's viewport, scroll bars excluded).
    pub fn set_page(&mut self, rect: Rect) {
        self.page_next = Some(rect);
    }

    /// End of the frame: the page set this frame takes the next frame's touches, unless
    /// something sits on top of it (`covered`: a dialog, menu or the File tab).
    pub fn end_frame(&mut self, covered: bool) {
        let next = self.page_next.take();
        self.page = if covered { None } else { next };
    }

    /// This frame's scroll from the fingers, a flick's coast included (content movement in
    /// screen points, like egui's wheel deltas). `dt`: seconds since the last frame.
    pub fn scroll(&mut self, dt: f32) -> Vec2 {
        let pan = std::mem::take(&mut self.pan);
        if pan != Vec2::ZERO {
            self.momentum.feed(pan, dt);
        }
        if std::mem::take(&mut self.lifted) {
            self.momentum.release();
        }
        if pan != Vec2::ZERO { pan } else { self.momentum.tick(dt) }
    }

    /// The page hit its end: stop coasting.
    pub fn hit_edge(&mut self) {
        self.momentum.cancel();
    }

    /// A finished swipe for Read Mode: `-1` turns back a page, `1` on, `0` none (too short).
    /// Taken once; a swipe left or up turns on, as the wheel's direction does.
    pub fn take_swipe(&mut self) -> i32 {
        let Some(d) = self.swipe.take() else { return 0 };
        let along = if d.x.abs() >= d.y.abs() { d.x } else { d.y };
        if along.abs() < PAGE_SWIPE { 0 } else { -(along.signum() as i32) }
    }

    /// Something is still to happen without new input: a click's release, or a coast.
    pub fn needs_repaint(&self) -> bool {
        !self.later.is_empty() || self.momentum.is_active() || self.pan != Vec2::ZERO
    }
}

/// The finger rested long enough to be a hold (`now`, `since`: input times; unknown means a tap).
fn held(since: Option<f64>, now: Option<f64>) -> bool {
    matches!((since, now), (Some(s), Some(n)) if n - s >= HOLD)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    const PAGE: Rect = Rect { min: Pos2 { x: 100.0, y: 100.0 }, max: Pos2 { x: 900.0, y: 700.0 } };

    fn gestures() -> TouchGestures {
        let mut g = TouchGestures::default();
        g.set_page(PAGE);
        g.end_frame(false);
        g
    }

    fn touch(id: u64, phase: TouchPhase, pos: Pos2) -> Event {
        Event::Touch { device_id: egui::TouchDeviceId(0), id: TouchId(id), phase, pos, force: None }
    }

    /// One frame's input as eframe's web runner reports a single finger: the made-up mouse event
    /// first, then the touch.
    fn finger(g: &mut TouchGestures, time: f64, phase: TouchPhase, pos: Pos2) -> Vec<Event> {
        let pointer = match phase {
            TouchPhase::Start => vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Default::default() }],
            TouchPhase::Move => vec![Event::PointerMoved(pos)],
            TouchPhase::End | TouchPhase::Cancel => {
                vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Default::default() }, Event::PointerGone]
            }
        };
        let mut events = pointer;
        events.push(touch(1, phase, pos));
        frame(g, time, events)
    }

    fn frame(g: &mut TouchGestures, time: f64, events: Vec<Event>) -> Vec<Event> {
        let mut raw = egui::RawInput { events, time: Some(time), ..Default::default() };
        g.hook(&mut raw, false);
        raw.events.retain(|e| !matches!(e, Event::Touch { .. }));
        raw.events
    }

    fn press(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
        Event::PointerButton { pos, button, pressed, modifiers: Default::default() }
    }

    #[test]
    fn a_swipe_scrolls_the_page_without_pressing_the_mouse() {
        let mut g = gestures();
        let at = pos2(400.0, 500.0);
        assert_eq!(finger(&mut g, 0.0, TouchPhase::Start, at), vec![], "nothing is pressed while the touch could still be a swipe");
        assert_eq!(finger(&mut g, 0.02, TouchPhase::Move, at + vec2(0.0, -4.0)), vec![], "inside the slop");
        assert_eq!(g.scroll(0.02), Vec2::ZERO);
        assert_eq!(finger(&mut g, 0.04, TouchPhase::Move, at + vec2(0.0, -30.0)), vec![]);
        assert_eq!(g.scroll(0.02), vec2(0.0, -30.0), "the page follows the finger from where it went down");
        finger(&mut g, 0.06, TouchPhase::Move, at + vec2(0.0, -50.0));
        assert_eq!(g.scroll(0.02), vec2(0.0, -20.0));
        assert_eq!(finger(&mut g, 0.08, TouchPhase::End, at + vec2(0.0, -50.0)), vec![], "lifting releases nothing: nothing was pressed");
        assert!(g.scroll(0.016).y < 0.0, "a flick coasts on");
        assert!(g.needs_repaint());
        assert_eq!(g.take_swipe(), 1, "a swipe up turns on in Read Mode");
        assert_eq!(g.take_swipe(), 0, "once");
    }

    #[test]
    fn a_finger_on_the_glass_stops_the_coast() {
        let mut g = gestures();
        let at = pos2(400.0, 500.0);
        finger(&mut g, 0.0, TouchPhase::Start, at);
        finger(&mut g, 0.02, TouchPhase::Move, at + vec2(0.0, -40.0));
        finger(&mut g, 0.04, TouchPhase::Move, at + vec2(0.0, -80.0));
        finger(&mut g, 0.06, TouchPhase::End, at + vec2(0.0, -80.0));
        g.scroll(0.02);
        assert_ne!(g.scroll(0.016), Vec2::ZERO);
        finger(&mut g, 0.1, TouchPhase::Start, at);
        assert_eq!(g.scroll(0.016), Vec2::ZERO);
    }

    #[test]
    fn a_resting_finger_does_not_coast() {
        let mut g = gestures();
        let at = pos2(400.0, 500.0);
        finger(&mut g, 0.0, TouchPhase::Start, at);
        finger(&mut g, 0.02, TouchPhase::Move, at + vec2(0.0, -60.0));
        g.scroll(0.02);
        for _ in 0..30 {
            assert_eq!(g.scroll(0.016), Vec2::ZERO, "the finger rests: the page stays");
        }
        finger(&mut g, 0.6, TouchPhase::End, at + vec2(0.0, -60.0));
        assert_eq!(g.scroll(0.016), Vec2::ZERO, "it lifted after resting: no fling");
    }

    #[test]
    fn a_tap_clicks_with_the_release_a_frame_later() {
        let mut g = gestures();
        let at = pos2(300.0, 300.0);
        assert_eq!(finger(&mut g, 0.0, TouchPhase::Start, at), vec![]);
        let wobble = at + vec2(3.0, 2.0);
        assert_eq!(
            finger(&mut g, 0.05, TouchPhase::End, wobble),
            vec![Event::PointerMoved(at), press(at, PointerButton::Primary, true)],
            "a click where the finger went down"
        );
        assert!(g.needs_repaint(), "the release still has to come");
        assert_eq!(frame(&mut g, 0.07, vec![]), vec![press(at, PointerButton::Primary, false), Event::PointerGone]);
        assert!(!g.needs_repaint());
        assert_eq!(g.scroll(0.016), Vec2::ZERO, "a tap doesn't scroll");
    }

    #[test]
    fn hold_then_lift_is_a_right_click() {
        let mut g = gestures();
        let at = pos2(300.0, 300.0);
        finger(&mut g, 0.0, TouchPhase::Start, at);
        assert_eq!(finger(&mut g, 0.8, TouchPhase::End, at), vec![Event::PointerMoved(at), press(at, PointerButton::Secondary, true)]);
        assert_eq!(frame(&mut g, 0.82, vec![]), vec![press(at, PointerButton::Secondary, false), Event::PointerGone]);
    }

    #[test]
    fn hold_then_drag_drags_the_mouse_from_where_the_finger_went_down() {
        let mut g = gestures();
        let at = pos2(300.0, 300.0);
        finger(&mut g, 0.0, TouchPhase::Start, at);
        let to = at + vec2(60.0, 0.0);
        assert_eq!(finger(&mut g, 0.7, TouchPhase::Move, to), vec![Event::PointerMoved(at), press(at, PointerButton::Primary, true)]);
        let further = to + vec2(40.0, 10.0);
        assert_eq!(
            finger(&mut g, 0.72, TouchPhase::Move, further),
            vec![Event::PointerMoved(to), Event::PointerMoved(further)],
            "the move a frame after the press"
        );
        assert_eq!(g.scroll(0.016), Vec2::ZERO, "dragging selects, it doesn't scroll");
        assert_eq!(finger(&mut g, 0.75, TouchPhase::End, further), vec![press(further, PointerButton::Primary, false), Event::PointerGone]);
        assert_eq!(frame(&mut g, 0.77, vec![]), vec![], "nothing left over");
    }

    #[test]
    fn a_cancelled_drag_lets_the_button_up() {
        let mut g = gestures();
        let at = pos2(300.0, 300.0);
        finger(&mut g, 0.0, TouchPhase::Start, at);
        finger(&mut g, 0.7, TouchPhase::Move, at + vec2(0.0, 40.0));
        let out = frame(&mut g, 0.75, vec![touch(1, TouchPhase::Cancel, at + vec2(0.0, 40.0))]);
        let to = at + vec2(0.0, 40.0);
        assert_eq!(
            out,
            vec![Event::PointerMoved(to), press(to, PointerButton::Primary, false), Event::PointerGone],
            "the held-back move, then the release"
        );
        let cancelled_tap = {
            finger(&mut g, 1.0, TouchPhase::Start, at);
            frame(&mut g, 1.05, vec![touch(1, TouchPhase::Cancel, at)])
        };
        assert_eq!(cancelled_tap, vec![], "a cancelled tap clicks nothing");
    }

    #[test]
    fn two_fingers_scroll_by_their_centre_and_never_click() {
        let mut g = gestures();
        let (a, b) = (pos2(300.0, 300.0), pos2(500.0, 300.0));
        finger(&mut g, 0.0, TouchPhase::Start, a);
        assert_eq!(frame(&mut g, 0.01, vec![touch(2, TouchPhase::Start, b)]), vec![]);
        frame(
            &mut g,
            0.03,
            vec![
                Event::PointerMoved(a + vec2(0.0, 20.0)),
                touch(1, TouchPhase::Move, a + vec2(0.0, 20.0)),
                touch(2, TouchPhase::Move, b + vec2(0.0, 20.0)),
            ],
        );
        assert_eq!(g.scroll(0.02), vec2(0.0, 20.0));
        assert_eq!(frame(&mut g, 0.05, vec![touch(2, TouchPhase::End, b + vec2(0.0, 20.0))]), vec![]);
        assert_eq!(finger(&mut g, 0.06, TouchPhase::End, a + vec2(0.0, 20.0)), vec![], "lifting the fingers after a pinch clicks nothing");
    }

    #[test]
    fn touches_off_the_page_and_drawing_fingers_stay_the_mouse() {
        let mut g = gestures();
        let ribbon = pos2(300.0, 40.0);
        let down = finger(&mut g, 0.0, TouchPhase::Start, ribbon);
        assert_eq!(down, vec![press(ribbon, PointerButton::Primary, true)]);
        assert_eq!(
            finger(&mut g, 0.1, TouchPhase::Move, pos2(300.0, 400.0)),
            vec![Event::PointerMoved(pos2(300.0, 400.0))],
            "even when it slides onto the page"
        );
        finger(&mut g, 0.2, TouchPhase::End, pos2(300.0, 400.0));
        assert_eq!(g.scroll(0.016), Vec2::ZERO);

        let on_page = pos2(300.0, 300.0);
        let mut raw = egui::RawInput {
            events: vec![press(on_page, PointerButton::Primary, true), touch(1, TouchPhase::Start, on_page)],
            time: Some(1.0),
            ..Default::default()
        };
        g.hook(&mut raw, true);
        assert!(raw.events.contains(&press(on_page, PointerButton::Primary, true)), "a pen draws with the finger");
    }

    /// A pen on an Android tablet: Chrome reports it as a pointer (eframe presses the mouse at
    /// once) and then also as a touch. The touch stays the pen's shadow, so the button comes up.
    #[test]
    fn a_pointer_already_pressed_keeps_its_touches_as_the_mouse() {
        let mut g = gestures();
        let at = pos2(300.0, 300.0);
        let to = at + vec2(80.0, 0.0);
        assert_eq!(frame(&mut g, 0.0, vec![press(at, PointerButton::Primary, true)]), vec![press(at, PointerButton::Primary, true)]);
        assert_eq!(frame(&mut g, 0.02, vec![touch(1, TouchPhase::Start, at)]), vec![]);
        assert_eq!(finger(&mut g, 0.04, TouchPhase::Move, to), vec![Event::PointerMoved(to)], "the pen drags as the mouse");
        assert_eq!(g.scroll(0.016), Vec2::ZERO);
        assert_eq!(frame(&mut g, 0.06, vec![press(to, PointerButton::Primary, false)]), vec![press(to, PointerButton::Primary, false)]);
        frame(&mut g, 0.08, vec![touch(1, TouchPhase::End, to)]);
        // Lifted: the next finger is a finger again.
        assert_eq!(finger(&mut g, 1.0, TouchPhase::Start, at), vec![]);
        finger(&mut g, 1.02, TouchPhase::Move, at + vec2(0.0, -40.0));
        assert_eq!(g.scroll(0.016), vec2(0.0, -40.0));
    }

    #[test]
    fn a_covered_or_missing_page_takes_no_gestures() {
        let mut g = TouchGestures::default();
        let at = pos2(300.0, 300.0);
        assert_eq!(finger(&mut g, 0.0, TouchPhase::Start, at), vec![press(at, PointerButton::Primary, true)], "no page drawn yet");
        finger(&mut g, 0.1, TouchPhase::End, at);
        g.set_page(PAGE);
        g.end_frame(true);
        assert_eq!(finger(&mut g, 1.0, TouchPhase::Start, at), vec![press(at, PointerButton::Primary, true)], "a dialog covers it");
    }

    #[test]
    fn the_mouse_passes_untouched() {
        let mut g = gestures();
        let at = pos2(300.0, 300.0);
        let events = vec![Event::PointerMoved(at), press(at, PointerButton::Primary, true)];
        assert_eq!(frame(&mut g, 0.0, events.clone()), events);
        assert_eq!(frame(&mut g, 0.1, vec![Event::PointerMoved(at + vec2(50.0, 0.0))]), vec![Event::PointerMoved(at + vec2(50.0, 0.0))]);
    }

    #[test]
    fn hostile_touches_never_panic() {
        let mut g = gestures();
        for (i, p) in [pos2(f32::NAN, 1.0), pos2(f32::INFINITY, 3.0), pos2(300.0, 300.0), pos2(-1e30, 1e30)].into_iter().enumerate() {
            for phase in [TouchPhase::Start, TouchPhase::Move, TouchPhase::Move, TouchPhase::End, TouchPhase::Cancel] {
                frame(&mut g, i as f64, vec![touch(i as u64, phase, p)]);
                frame(&mut g, f64::NAN, vec![touch(99, phase, p)]);
            }
        }
        for id in 0..100 {
            frame(&mut g, 0.0, vec![touch(id, TouchPhase::Start, pos2(300.0, 300.0))]);
        }
        assert!(g.fingers.len() <= MAX_FINGERS);
        assert!(g.scroll(f32::NAN).is_finite());
    }
}
