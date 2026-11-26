//! Pure USB HID keyboard state tracker for Nintendo Switch (`libnx` `HidKeyboardState`).

use std::time::{Duration, Instant};

/// `HidKeyboardModifier` bitflags (`switch/services/hid.h`).
pub mod kbd_modifiers {
    pub const CONTROL: u64 = 1 << 0;
    pub const SHIFT: u64 = 1 << 1;
    pub const LEFT_ALT: u64 = 1 << 2;
    pub const RIGHT_ALT: u64 = 1 << 3;
    pub const GUI: u64 = 1 << 4;
    pub const CAPS_LOCK: u64 = 1 << 8;
    pub const SCROLL_LOCK: u64 = 1 << 9;
    pub const NUM_LOCK: u64 = 1 << 10;
}

const ACTIVE_MOD_MASK: u64 = kbd_modifiers::CONTROL
    | kbd_modifiers::SHIFT
    | kbd_modifiers::LEFT_ALT
    | kbd_modifiers::RIGHT_ALT
    | kbd_modifiers::GUI;

const LEFT_CONTROL: u8 = 224;
const LEFT_SHIFT: u8 = 225;
const LEFT_ALT: u8 = 226;
const LEFT_GUI: u8 = 227;
const RIGHT_CONTROL: u8 = 228;
const RIGHT_SHIFT: u8 = 229;
const RIGHT_ALT: u8 = 230;
const RIGHT_GUI: u8 = 231;

const REPEAT_DELAY: Duration = Duration::from_millis(400);
const REPEAT_INTERVAL: Duration = Duration::from_millis(45);

/// Plain-data representation of `HidKeyboardState` for host-testable tracking.
#[derive(Default, Copy, Clone, Debug, PartialEq, Eq)]
pub struct RawKeyboardSample {
    pub sampling_number: u64,
    pub modifiers: u64,
    pub keys: [u64; 4],
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct KeyChange {
    pub usage: u8,
    pub pressed: bool,
    pub repeat: bool,
    pub modifiers: u64,
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct KeyboardFrame {
    pub modifiers: u64,
    pub modifiers_changed: bool,
    /// Bitmask matching winit's `ModifiersKeys` (`LSHIFT=1<<0 .. RSUPER=1<<7`).
    pub pressed_modifier_keys: u8,
    pub events: Vec<KeyChange>,
}

#[derive(Copy, Clone, Debug)]
struct RepeatState {
    usage: u8,
    next_repeat: Instant,
}

/// Tracks `HidKeyboardState` LIFO samples (newest at index 0) and emits chronological key
/// press/release events, modifier changes, and key repeats.
#[derive(Default)]
pub struct KeyboardTracker {
    last_sampling_number: Option<u64>,
    held: [u64; 4],
    last_modifiers: u64,
    last_pressed_modifier_keys: u8,
    repeat: Option<RepeatState>,
}

impl KeyboardTracker {
    pub fn update(&mut self, samples_newest_first: &[RawKeyboardSample]) -> KeyboardFrame {
        self.update_at(samples_newest_first, Instant::now())
    }

    pub fn update_at(
        &mut self,
        samples_newest_first: &[RawKeyboardSample],
        now: Instant,
    ) -> KeyboardFrame {
        if self.last_sampling_number.is_none() {
            let Some(&newest) = samples_newest_first.first() else {
                return KeyboardFrame::default();
            };
            let (keys, mods, pressed_mod_keys) = normalize_sample(newest);
            self.last_sampling_number = Some(newest.sampling_number);
            self.held = keys;
            self.last_modifiers = mods;
            self.last_pressed_modifier_keys = pressed_mod_keys;
            return KeyboardFrame::default();
        }

        let last_seq = self.last_sampling_number.unwrap_or(0);
        let prev_mods = self.last_modifiers;
        let mut events = Vec::new();

        for s in samples_newest_first.iter().rev() {
            if s.sampling_number <= last_seq {
                continue;
            }
            self.last_sampling_number = Some(s.sampling_number);
            let (cur_keys, cur_mods, pressed_mod_keys) = normalize_sample(*s);
            self.last_modifiers = cur_mods;
            self.last_pressed_modifier_keys = pressed_mod_keys;

            for word_idx in 0..4 {
                let mut released = self.held[word_idx] & !cur_keys[word_idx];
                while released != 0 {
                    let bit = released.trailing_zeros() as usize;
                    released &= released - 1;
                    let usage = (word_idx * 64 + bit) as u8;
                    if usage <= RIGHT_GUI {
                        if self.repeat.is_some_and(|r| r.usage == usage) {
                            self.repeat = None;
                        }
                        events.push(KeyChange {
                            usage,
                            pressed: false,
                            repeat: false,
                            modifiers: cur_mods,
                        });
                    }
                }
            }

            for word_idx in 0..4 {
                let mut pressed = cur_keys[word_idx] & !self.held[word_idx];
                while pressed != 0 {
                    let bit = pressed.trailing_zeros() as usize;
                    pressed &= pressed - 1;
                    let usage = (word_idx * 64 + bit) as u8;
                    if usage <= RIGHT_GUI {
                        if !is_modifier_usage(usage) {
                            self.repeat = Some(RepeatState {
                                usage,
                                next_repeat: now + REPEAT_DELAY,
                            });
                        }
                        events.push(KeyChange {
                            usage,
                            pressed: true,
                            repeat: false,
                            modifiers: cur_mods,
                        });
                    }
                }
            }

            self.held = cur_keys;
        }

        if let Some(rep) = &mut self.repeat
            && has_usage(&self.held, rep.usage)
        {
            if now >= rep.next_repeat {
                events.push(KeyChange {
                    usage: rep.usage,
                    pressed: true,
                    repeat: true,
                    modifiers: self.last_modifiers,
                });
                rep.next_repeat = now + REPEAT_INTERVAL;
            }
        } else {
            self.repeat = None;
        }

        KeyboardFrame {
            modifiers: self.last_modifiers,
            modifiers_changed: (self.last_modifiers & ACTIVE_MOD_MASK)
                != (prev_mods & ACTIVE_MOD_MASK),
            pressed_modifier_keys: self.last_pressed_modifier_keys,
            events,
        }
    }
}

fn is_modifier_usage(usage: u8) -> bool {
    (LEFT_CONTROL..=RIGHT_GUI).contains(&usage)
}

fn has_usage(keys: &[u64; 4], usage: u8) -> bool {
    let idx = (usage as usize) / 64;
    let bit = (usage as usize) % 64;
    (keys[idx] & (1u64 << bit)) != 0
}

fn set_usage(keys: &mut [u64; 4], usage: u8) {
    let idx = (usage as usize) / 64;
    let bit = (usage as usize) % 64;
    keys[idx] |= 1u64 << bit;
}

fn normalize_sample(sample: RawKeyboardSample) -> ([u64; 4], u64, u8) {
    let mut keys = sample.keys;
    // Ignore HID usages 0..=3 (None, ErrorRollOver, POSTFail, ErrorUndefined) and > 231.
    keys[0] &= !0x0F;
    keys[3] &= (1u64 << 40) - 1;

    let mut mods = sample.modifiers;
    if (mods & kbd_modifiers::CONTROL) != 0
        && !has_usage(&keys, LEFT_CONTROL)
        && !has_usage(&keys, RIGHT_CONTROL)
    {
        set_usage(&mut keys, LEFT_CONTROL);
    }
    if (mods & kbd_modifiers::SHIFT) != 0
        && !has_usage(&keys, LEFT_SHIFT)
        && !has_usage(&keys, RIGHT_SHIFT)
    {
        set_usage(&mut keys, LEFT_SHIFT);
    }
    if (mods & kbd_modifiers::LEFT_ALT) != 0 && !has_usage(&keys, LEFT_ALT) {
        set_usage(&mut keys, LEFT_ALT);
    }
    if (mods & kbd_modifiers::RIGHT_ALT) != 0 && !has_usage(&keys, RIGHT_ALT) {
        set_usage(&mut keys, RIGHT_ALT);
    }
    if (mods & kbd_modifiers::GUI) != 0
        && !has_usage(&keys, LEFT_GUI)
        && !has_usage(&keys, RIGHT_GUI)
    {
        set_usage(&mut keys, LEFT_GUI);
    }

    if has_usage(&keys, LEFT_CONTROL) || has_usage(&keys, RIGHT_CONTROL) {
        mods |= kbd_modifiers::CONTROL;
    }
    if has_usage(&keys, LEFT_SHIFT) || has_usage(&keys, RIGHT_SHIFT) {
        mods |= kbd_modifiers::SHIFT;
    }
    if has_usage(&keys, LEFT_ALT) {
        mods |= kbd_modifiers::LEFT_ALT;
    }
    if has_usage(&keys, RIGHT_ALT) {
        mods |= kbd_modifiers::RIGHT_ALT;
    }
    if has_usage(&keys, LEFT_GUI) || has_usage(&keys, RIGHT_GUI) {
        mods |= kbd_modifiers::GUI;
    }

    let mut pressed_mod_keys = 0u8;
    if has_usage(&keys, LEFT_SHIFT) {
        pressed_mod_keys |= 1 << 0;
    }
    if has_usage(&keys, RIGHT_SHIFT) {
        pressed_mod_keys |= 1 << 1;
    }
    if has_usage(&keys, LEFT_CONTROL) {
        pressed_mod_keys |= 1 << 2;
    }
    if has_usage(&keys, RIGHT_CONTROL) {
        pressed_mod_keys |= 1 << 3;
    }
    if has_usage(&keys, LEFT_ALT) {
        pressed_mod_keys |= 1 << 4;
    }
    if has_usage(&keys, RIGHT_ALT) {
        pressed_mod_keys |= 1 << 5;
    }
    if has_usage(&keys, LEFT_GUI) {
        pressed_mod_keys |= 1 << 6;
    }
    if has_usage(&keys, RIGHT_GUI) {
        pressed_mod_keys |= 1 << 7;
    }

    (keys, mods, pressed_mod_keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_with(seq: u64, mods: u64, usages: &[u8]) -> RawKeyboardSample {
        let mut keys = [0u64; 4];
        for &u in usages {
            set_usage(&mut keys, u);
        }
        RawKeyboardSample {
            sampling_number: seq,
            modifiers: mods,
            keys,
        }
    }

    #[test]
    fn ignores_rollover_and_diffs_chronologically() {
        let mut tracker = KeyboardTracker::default();
        let t0 = Instant::now();
        assert!(tracker.update_at(&[sample_with(1, 0, &[])], t0).events.is_empty());

        // Newest first: seq 3 releases A and presses B with RollOver (1), seq 2 presses A.
        let frame = tracker.update_at(
            &[
                sample_with(3, kbd_modifiers::SHIFT, &[1, 5]),
                sample_with(2, 0, &[4]),
            ],
            t0 + Duration::from_millis(10),
        );
        assert!(frame.modifiers_changed);
        assert_eq!(frame.pressed_modifier_keys, 1 << 0);
        assert_eq!(
            frame.events,
            vec![
                KeyChange {
                    usage: 4,
                    pressed: true,
                    repeat: false,
                    modifiers: 0,
                },
                KeyChange {
                    usage: 4,
                    pressed: false,
                    repeat: false,
                    modifiers: kbd_modifiers::SHIFT,
                },
                KeyChange {
                    usage: 5,
                    pressed: true,
                    repeat: false,
                    modifiers: kbd_modifiers::SHIFT,
                },
                KeyChange {
                    usage: LEFT_SHIFT,
                    pressed: true,
                    repeat: false,
                    modifiers: kbd_modifiers::SHIFT,
                },
            ]
        );
    }

    #[test]
    fn key_repeat_fires_and_cancels_on_release() {
        let mut tracker = KeyboardTracker::default();
        let t0 = Instant::now();
        tracker.update_at(&[sample_with(1, 0, &[])], t0);
        let held = sample_with(2, 0, &[26]); // W
        let f1 = tracker.update_at(&[held], t0 + Duration::from_millis(10));
        assert_eq!(f1.events.len(), 1);
        assert!(!f1.events[0].repeat);

        // Before 400ms: no repeat.
        let f2 = tracker.update_at(&[held], t0 + Duration::from_millis(300));
        assert!(f2.events.is_empty());

        // Past 400ms: repeat fires even when sampling_number hasn't changed.
        let f3 = tracker.update_at(&[held], t0 + Duration::from_millis(420));
        assert_eq!(
            f3.events,
            vec![KeyChange {
                usage: 26,
                pressed: true,
                repeat: true,
                modifiers: 0,
            }]
        );

        // Release cancels repeat.
        let f4 = tracker.update_at(&[sample_with(3, 0, &[])], t0 + Duration::from_millis(470));
        assert_eq!(
            f4.events,
            vec![KeyChange {
                usage: 26,
                pressed: false,
                repeat: false,
                modifiers: 0,
            }]
        );
        let f5 = tracker.update_at(&[], t0 + Duration::from_millis(600));
        assert!(f5.events.is_empty());
    }
}
