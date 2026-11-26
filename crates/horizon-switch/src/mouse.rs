//! Pure USB HID mouse state tracker for Nintendo Switch (`libnx` `HidMouseState`).

/// `HidMouseButton` bitflags (`switch/services/hid.h`).
pub mod mouse_buttons {
    pub const LEFT: u32 = 1 << 0;
    pub const RIGHT: u32 = 1 << 1;
    pub const MIDDLE: u32 = 1 << 2;
    pub const FORWARD: u32 = 1 << 3;
    pub const BACK: u32 = 1 << 4;
}

/// `HidMouseAttribute` bitflags (`switch/services/hid.h`).
pub mod mouse_attrs {
    pub const TRANSFERABLE: u32 = 1 << 0;
    pub const IS_CONNECTED: u32 = 1 << 1;
}

/// Plain-data representation of `HidMouseState` for host-testable tracking.
#[derive(Default, Copy, Clone, Debug, PartialEq, Eq)]
pub struct RawMouseSample {
    pub sampling_number: u64,
    pub delta_x: i32,
    pub delta_y: i32,
    pub wheel_delta_x: i32,
    pub wheel_delta_y: i32,
    pub buttons: u32,
    pub attributes: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MouseButtonId {
    Left,
    Right,
    Middle,
    Forward,
    Back,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MouseEvent {
    Motion { dx: i32, dy: i32 },
    Wheel { dx: i32, dy: i32 },
    Button { button: MouseButtonId, pressed: bool },
}

const BUTTON_MAP: [(u32, MouseButtonId); 5] = [
    (mouse_buttons::LEFT, MouseButtonId::Left),
    (mouse_buttons::RIGHT, MouseButtonId::Right),
    (mouse_buttons::MIDDLE, MouseButtonId::Middle),
    (mouse_buttons::FORWARD, MouseButtonId::Forward),
    (mouse_buttons::BACK, MouseButtonId::Back),
];

/// Tracks `HidMouseState` LIFO samples (newest at index 0) and emits chronological motion,
/// wheel, and button-edge events.
#[derive(Default)]
pub struct MouseTracker {
    last_sampling_number: Option<u64>,
    last_buttons: u32,
    connected: bool,
}

impl MouseTracker {
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub fn update(&mut self, samples_newest_first: &[RawMouseSample]) -> Vec<MouseEvent> {
        let Some(&newest) = samples_newest_first.first() else {
            return Vec::new();
        };
        let Some(last_seq) = self.last_sampling_number else {
            let connected = (newest.attributes & mouse_attrs::IS_CONNECTED) != 0;
            self.last_sampling_number = Some(newest.sampling_number);
            self.connected = connected;
            self.last_buttons = if connected { newest.buttons } else { 0 };
            return Vec::new();
        };

        let mut out = Vec::new();
        let mut dx: i32 = 0;
        let mut dy: i32 = 0;
        let mut wheel_x: i32 = 0;
        let mut wheel_y: i32 = 0;

        for s in samples_newest_first.iter().rev() {
            if s.sampling_number <= last_seq {
                continue;
            }
            self.last_sampling_number = Some(s.sampling_number);
            self.connected = (s.attributes & mouse_attrs::IS_CONNECTED) != 0;
            dx = dx.saturating_add(s.delta_x);
            dy = dy.saturating_add(s.delta_y);
            wheel_x = wheel_x.saturating_add(s.wheel_delta_x);
            wheel_y = wheel_y.saturating_add(s.wheel_delta_y);

            let cur_buttons = if self.connected { s.buttons } else { 0 };
            if cur_buttons != self.last_buttons {
                flush_motion_and_wheel(&mut out, &mut dx, &mut dy, &mut wheel_x, &mut wheel_y);
                let changed = cur_buttons ^ self.last_buttons;
                for &(bit, button) in &BUTTON_MAP {
                    if (changed & bit) != 0 {
                        out.push(MouseEvent::Button {
                            button,
                            pressed: (cur_buttons & bit) != 0,
                        });
                    }
                }
                self.last_buttons = cur_buttons;
            }
        }

        flush_motion_and_wheel(&mut out, &mut dx, &mut dy, &mut wheel_x, &mut wheel_y);
        out
    }
}

fn flush_motion_and_wheel(
    out: &mut Vec<MouseEvent>,
    dx: &mut i32,
    dy: &mut i32,
    wheel_x: &mut i32,
    wheel_y: &mut i32,
) {
    if *dx != 0 || *dy != 0 {
        out.push(MouseEvent::Motion { dx: *dx, dy: *dy });
        *dx = 0;
        *dy = 0;
    }
    if *wheel_x != 0 || *wheel_y != 0 {
        out.push(MouseEvent::Wheel {
            dx: *wheel_x,
            dy: *wheel_y,
        });
        *wheel_x = 0;
        *wheel_y = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(seq: u64, dx: i32, dy: i32, wy: i32, buttons: u32, connected: bool) -> RawMouseSample {
        RawMouseSample {
            sampling_number: seq,
            delta_x: dx,
            delta_y: dy,
            wheel_delta_x: 0,
            wheel_delta_y: wy,
            buttons,
            attributes: if connected { mouse_attrs::IS_CONNECTED } else { 0 },
        }
    }

    #[test]
    fn first_poll_seeds_without_emitting() {
        let mut tracker = MouseTracker::default();
        let events = tracker.update(&[sample(10, 50, -30, 1, mouse_buttons::LEFT, true)]);
        assert!(events.is_empty());
        assert!(tracker.is_connected());
    }

    #[test]
    fn sums_chronological_motion_and_splits_at_button_edges() {
        let mut tracker = MouseTracker::default();
        tracker.update(&[sample(1, 0, 0, 0, 0, true)]);

        // Newest first: seq 4, seq 3, seq 2, seq 1 (already seen).
        let events = tracker.update(&[
            sample(4, 3, -1, 0, mouse_buttons::LEFT, true),
            sample(3, 4, 2, 1, mouse_buttons::LEFT, true),
            sample(2, 5, 6, 0, 0, true),
            sample(1, 99, 99, 99, 0, true),
        ]);

        assert_eq!(
            events,
            vec![
                MouseEvent::Motion { dx: 9, dy: 8 },
                MouseEvent::Wheel { dx: 0, dy: 1 },
                MouseEvent::Button {
                    button: MouseButtonId::Left,
                    pressed: true,
                },
                MouseEvent::Motion { dx: 3, dy: -1 },
            ]
        );
    }

    #[test]
    fn disconnect_releases_held_buttons() {
        let mut tracker = MouseTracker::default();
        tracker.update(&[sample(1, 0, 0, 0, mouse_buttons::LEFT | mouse_buttons::RIGHT, true)]);
        let events = tracker.update(&[sample(
            2,
            0,
            0,
            0,
            mouse_buttons::LEFT | mouse_buttons::RIGHT,
            false,
        )]);
        assert!(!tracker.is_connected());
        assert_eq!(
            events,
            vec![
                MouseEvent::Button {
                    button: MouseButtonId::Left,
                    pressed: false,
                },
                MouseEvent::Button {
                    button: MouseButtonId::Right,
                    pressed: false,
                },
            ]
        );
    }
}
