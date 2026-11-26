use horizon_switch::{KeyChange, kbd_modifiers};
use smol_str::SmolStr;

use super::KeyEventExtra;
use crate::event::{ElementState, KeyEvent, Modifiers};
use crate::keyboard::{
    Key, KeyCode, KeyLocation, ModifiersKeys, ModifiersState, NamedKey, PhysicalKey,
};

pub(super) fn modifiers_from_frame(modifiers: u64, pressed_modifier_keys: u8) -> Modifiers {
    let mut state = ModifiersState::empty();
    if (modifiers & kbd_modifiers::CONTROL) != 0 {
        state |= ModifiersState::CONTROL;
    }
    if (modifiers & kbd_modifiers::SHIFT) != 0 {
        state |= ModifiersState::SHIFT;
    }
    if (modifiers & (kbd_modifiers::LEFT_ALT | kbd_modifiers::RIGHT_ALT)) != 0 {
        state |= ModifiersState::ALT;
    }
    if (modifiers & kbd_modifiers::GUI) != 0 {
        state |= ModifiersState::SUPER;
    }
    Modifiers {
        state,
        pressed_mods: ModifiersKeys::from_bits_truncate(pressed_modifier_keys),
    }
}

pub(super) fn to_key_event(change: KeyChange) -> Option<KeyEvent> {
    let (code, location) = usage_to_code_and_location(change.usage)?;
    let shift = (change.modifiers & kbd_modifiers::SHIFT) != 0;
    let caps = (change.modifiers & kbd_modifiers::CAPS_LOCK) != 0;
    let ctrl = (change.modifiers & kbd_modifiers::CONTROL) != 0;
    let alt = (change.modifiers & (kbd_modifiers::LEFT_ALT | kbd_modifiers::RIGHT_ALT)) != 0;
    let gui = (change.modifiers & kbd_modifiers::GUI) != 0;
    let num_lock = (change.modifiers & kbd_modifiers::NUM_LOCK) != 0;

    let (logical_key, key_without_modifiers, raw_text, ctrl_char) =
        map_keys(change.usage, shift, caps, num_lock)?;

    let (text, text_with_all_modifiers) = if change.pressed {
        let normal_text = if !ctrl && !alt && !gui {
            raw_text.clone()
        } else if matches!(
            logical_key,
            Key::Named(NamedKey::Enter | NamedKey::Tab | NamedKey::Backspace | NamedKey::Delete)
        ) {
            raw_text.clone()
        } else {
            None
        };
        let all_mods_text = if ctrl && !alt && !gui {
            ctrl_char.and_then(char_smol).or_else(|| normal_text.clone())
        } else {
            normal_text.clone()
        };
        (normal_text, all_mods_text)
    } else {
        (None, None)
    };

    Some(KeyEvent {
        physical_key: PhysicalKey::Code(code),
        logical_key,
        text,
        location,
        state: if change.pressed {
            ElementState::Pressed
        } else {
            ElementState::Released
        },
        repeat: change.repeat,
        platform_specific: KeyEventExtra {
            key_without_modifiers,
            text_with_all_modifiers,
        },
    })
}

fn char_smol(ch: char) -> Option<SmolStr> {
    let mut buf = [0u8; 4];
    Some(SmolStr::new(ch.encode_utf8(&mut buf)))
}

fn char_key(ch: char) -> Key {
    let mut buf = [0u8; 4];
    Key::Character(SmolStr::new(ch.encode_utf8(&mut buf)))
}

fn map_keys(
    usage: u8,
    shift: bool,
    caps: bool,
    num_lock: bool,
) -> Option<(Key, Key, Option<SmolStr>, Option<char>)> {
    if (4..=29).contains(&usage) {
        let idx = usage - 4;
        let lower = (b'a' + idx) as char;
        let upper = (b'A' + idx) as char;
        let active = if shift ^ caps { upper } else { lower };
        let ctrl_c = (idx + 1) as char;
        return Some((char_key(active), char_key(lower), char_smol(active), Some(ctrl_c)));
    }

    let pair = match usage {
        30 => ('1', '!'),
        31 => ('2', '@'),
        32 => ('3', '#'),
        33 => ('4', '$'),
        34 => ('5', '%'),
        35 => ('6', '^'),
        36 => ('7', '&'),
        37 => ('8', '*'),
        38 => ('9', '('),
        39 => ('0', ')'),
        45 => ('-', '_'),
        46 => ('=', '+'),
        47 => ('[', '{'),
        48 => (']', '}'),
        49 | 50 => ('\\', '|'),
        51 => (';', ':'),
        52 => ('\'', '"'),
        53 => ('`', '~'),
        54 => (',', '<'),
        55 => ('.', '>'),
        56 => ('/', '?'),
        100 => ('\\', '|'),
        _ => ('\0', '\0'),
    };
    if pair.0 != '\0' {
        let active = if shift { pair.1 } else { pair.0 };
        return Some((char_key(active), char_key(pair.0), char_smol(active), None));
    }

    if let Some((digit, nav)) = match usage {
        89 => Some(('1', NamedKey::End)),
        90 => Some(('2', NamedKey::ArrowDown)),
        91 => Some(('3', NamedKey::PageDown)),
        92 => Some(('4', NamedKey::ArrowLeft)),
        93 => Some(('5', NamedKey::Clear)),
        94 => Some(('6', NamedKey::ArrowRight)),
        95 => Some(('7', NamedKey::Home)),
        96 => Some(('8', NamedKey::ArrowUp)),
        97 => Some(('9', NamedKey::PageUp)),
        98 => Some(('0', NamedKey::Insert)),
        99 => Some(('.', NamedKey::Delete)),
        _ => None,
    } {
        if num_lock {
            return Some((char_key(digit), char_key(digit), char_smol(digit), None));
        }
        let k = Key::Named(nav);
        let text = if nav == NamedKey::Delete {
            Some(SmolStr::new_inline("\u{007f}"))
        } else {
            None
        };
        return Some((k.clone(), k, text, None));
    }

    if let Some(ch) = match usage {
        84 => Some('/'),
        85 => Some('*'),
        86 => Some('-'),
        87 => Some('+'),
        103 => Some('='),
        _ => None,
    } {
        return Some((char_key(ch), char_key(ch), char_smol(ch), None));
    }

    let (named, text) = match usage {
        40 | 88 => (NamedKey::Enter, Some(SmolStr::new_inline("\r"))),
        41 => (NamedKey::Escape, None),
        42 => (NamedKey::Backspace, Some(SmolStr::new_inline("\u{0008}"))),
        43 => (NamedKey::Tab, Some(SmolStr::new_inline("\t"))),
        44 => (NamedKey::Space, Some(SmolStr::new_inline(" "))),
        57 => (NamedKey::CapsLock, None),
        58 => (NamedKey::F1, None),
        59 => (NamedKey::F2, None),
        60 => (NamedKey::F3, None),
        61 => (NamedKey::F4, None),
        62 => (NamedKey::F5, None),
        63 => (NamedKey::F6, None),
        64 => (NamedKey::F7, None),
        65 => (NamedKey::F8, None),
        66 => (NamedKey::F9, None),
        67 => (NamedKey::F10, None),
        68 => (NamedKey::F11, None),
        69 => (NamedKey::F12, None),
        70 => (NamedKey::PrintScreen, None),
        71 => (NamedKey::ScrollLock, None),
        72 => (NamedKey::Pause, None),
        73 => (NamedKey::Insert, None),
        74 => (NamedKey::Home, None),
        75 => (NamedKey::PageUp, None),
        76 => (NamedKey::Delete, Some(SmolStr::new_inline("\u{007f}"))),
        77 => (NamedKey::End, None),
        78 => (NamedKey::PageDown, None),
        79 => (NamedKey::ArrowRight, None),
        80 => (NamedKey::ArrowLeft, None),
        81 => (NamedKey::ArrowDown, None),
        82 => (NamedKey::ArrowUp, None),
        83 => (NamedKey::NumLock, None),
        101 => (NamedKey::ContextMenu, None),
        104 => (NamedKey::F13, None),
        105 => (NamedKey::F14, None),
        106 => (NamedKey::F15, None),
        107 => (NamedKey::F16, None),
        108 => (NamedKey::F17, None),
        109 => (NamedKey::F18, None),
        110 => (NamedKey::F19, None),
        111 => (NamedKey::F20, None),
        112 => (NamedKey::F21, None),
        113 => (NamedKey::F22, None),
        114 => (NamedKey::F23, None),
        115 => (NamedKey::F24, None),
        224 | 228 => (NamedKey::Control, None),
        225 | 229 => (NamedKey::Shift, None),
        226 | 230 => (NamedKey::Alt, None),
        227 | 231 => (NamedKey::Super, None),
        _ => return None,
    };

    let key = Key::Named(named);
    Some((key.clone(), key, text, None))
}

fn usage_to_code_and_location(usage: u8) -> Option<(KeyCode, KeyLocation)> {
    Some(match usage {
        4 => (KeyCode::KeyA, KeyLocation::Standard),
        5 => (KeyCode::KeyB, KeyLocation::Standard),
        6 => (KeyCode::KeyC, KeyLocation::Standard),
        7 => (KeyCode::KeyD, KeyLocation::Standard),
        8 => (KeyCode::KeyE, KeyLocation::Standard),
        9 => (KeyCode::KeyF, KeyLocation::Standard),
        10 => (KeyCode::KeyG, KeyLocation::Standard),
        11 => (KeyCode::KeyH, KeyLocation::Standard),
        12 => (KeyCode::KeyI, KeyLocation::Standard),
        13 => (KeyCode::KeyJ, KeyLocation::Standard),
        14 => (KeyCode::KeyK, KeyLocation::Standard),
        15 => (KeyCode::KeyL, KeyLocation::Standard),
        16 => (KeyCode::KeyM, KeyLocation::Standard),
        17 => (KeyCode::KeyN, KeyLocation::Standard),
        18 => (KeyCode::KeyO, KeyLocation::Standard),
        19 => (KeyCode::KeyP, KeyLocation::Standard),
        20 => (KeyCode::KeyQ, KeyLocation::Standard),
        21 => (KeyCode::KeyR, KeyLocation::Standard),
        22 => (KeyCode::KeyS, KeyLocation::Standard),
        23 => (KeyCode::KeyT, KeyLocation::Standard),
        24 => (KeyCode::KeyU, KeyLocation::Standard),
        25 => (KeyCode::KeyV, KeyLocation::Standard),
        26 => (KeyCode::KeyW, KeyLocation::Standard),
        27 => (KeyCode::KeyX, KeyLocation::Standard),
        28 => (KeyCode::KeyY, KeyLocation::Standard),
        29 => (KeyCode::KeyZ, KeyLocation::Standard),
        30 => (KeyCode::Digit1, KeyLocation::Standard),
        31 => (KeyCode::Digit2, KeyLocation::Standard),
        32 => (KeyCode::Digit3, KeyLocation::Standard),
        33 => (KeyCode::Digit4, KeyLocation::Standard),
        34 => (KeyCode::Digit5, KeyLocation::Standard),
        35 => (KeyCode::Digit6, KeyLocation::Standard),
        36 => (KeyCode::Digit7, KeyLocation::Standard),
        37 => (KeyCode::Digit8, KeyLocation::Standard),
        38 => (KeyCode::Digit9, KeyLocation::Standard),
        39 => (KeyCode::Digit0, KeyLocation::Standard),
        40 => (KeyCode::Enter, KeyLocation::Standard),
        41 => (KeyCode::Escape, KeyLocation::Standard),
        42 => (KeyCode::Backspace, KeyLocation::Standard),
        43 => (KeyCode::Tab, KeyLocation::Standard),
        44 => (KeyCode::Space, KeyLocation::Standard),
        45 => (KeyCode::Minus, KeyLocation::Standard),
        46 => (KeyCode::Equal, KeyLocation::Standard),
        47 => (KeyCode::BracketLeft, KeyLocation::Standard),
        48 => (KeyCode::BracketRight, KeyLocation::Standard),
        49 | 50 => (KeyCode::Backslash, KeyLocation::Standard),
        51 => (KeyCode::Semicolon, KeyLocation::Standard),
        52 => (KeyCode::Quote, KeyLocation::Standard),
        53 => (KeyCode::Backquote, KeyLocation::Standard),
        54 => (KeyCode::Comma, KeyLocation::Standard),
        55 => (KeyCode::Period, KeyLocation::Standard),
        56 => (KeyCode::Slash, KeyLocation::Standard),
        57 => (KeyCode::CapsLock, KeyLocation::Standard),
        58 => (KeyCode::F1, KeyLocation::Standard),
        59 => (KeyCode::F2, KeyLocation::Standard),
        60 => (KeyCode::F3, KeyLocation::Standard),
        61 => (KeyCode::F4, KeyLocation::Standard),
        62 => (KeyCode::F5, KeyLocation::Standard),
        63 => (KeyCode::F6, KeyLocation::Standard),
        64 => (KeyCode::F7, KeyLocation::Standard),
        65 => (KeyCode::F8, KeyLocation::Standard),
        66 => (KeyCode::F9, KeyLocation::Standard),
        67 => (KeyCode::F10, KeyLocation::Standard),
        68 => (KeyCode::F11, KeyLocation::Standard),
        69 => (KeyCode::F12, KeyLocation::Standard),
        70 => (KeyCode::PrintScreen, KeyLocation::Standard),
        71 => (KeyCode::ScrollLock, KeyLocation::Standard),
        72 => (KeyCode::Pause, KeyLocation::Standard),
        73 => (KeyCode::Insert, KeyLocation::Standard),
        74 => (KeyCode::Home, KeyLocation::Standard),
        75 => (KeyCode::PageUp, KeyLocation::Standard),
        76 => (KeyCode::Delete, KeyLocation::Standard),
        77 => (KeyCode::End, KeyLocation::Standard),
        78 => (KeyCode::PageDown, KeyLocation::Standard),
        79 => (KeyCode::ArrowRight, KeyLocation::Standard),
        80 => (KeyCode::ArrowLeft, KeyLocation::Standard),
        81 => (KeyCode::ArrowDown, KeyLocation::Standard),
        82 => (KeyCode::ArrowUp, KeyLocation::Standard),
        83 => (KeyCode::NumLock, KeyLocation::Standard),
        84 => (KeyCode::NumpadDivide, KeyLocation::Numpad),
        85 => (KeyCode::NumpadMultiply, KeyLocation::Numpad),
        86 => (KeyCode::NumpadSubtract, KeyLocation::Numpad),
        87 => (KeyCode::NumpadAdd, KeyLocation::Numpad),
        88 => (KeyCode::NumpadEnter, KeyLocation::Numpad),
        89 => (KeyCode::Numpad1, KeyLocation::Numpad),
        90 => (KeyCode::Numpad2, KeyLocation::Numpad),
        91 => (KeyCode::Numpad3, KeyLocation::Numpad),
        92 => (KeyCode::Numpad4, KeyLocation::Numpad),
        93 => (KeyCode::Numpad5, KeyLocation::Numpad),
        94 => (KeyCode::Numpad6, KeyLocation::Numpad),
        95 => (KeyCode::Numpad7, KeyLocation::Numpad),
        96 => (KeyCode::Numpad8, KeyLocation::Numpad),
        97 => (KeyCode::Numpad9, KeyLocation::Numpad),
        98 => (KeyCode::Numpad0, KeyLocation::Numpad),
        99 => (KeyCode::NumpadDecimal, KeyLocation::Numpad),
        100 => (KeyCode::IntlBackslash, KeyLocation::Standard),
        101 => (KeyCode::ContextMenu, KeyLocation::Standard),
        103 => (KeyCode::NumpadEqual, KeyLocation::Numpad),
        104 => (KeyCode::F13, KeyLocation::Standard),
        105 => (KeyCode::F14, KeyLocation::Standard),
        106 => (KeyCode::F15, KeyLocation::Standard),
        107 => (KeyCode::F16, KeyLocation::Standard),
        108 => (KeyCode::F17, KeyLocation::Standard),
        109 => (KeyCode::F18, KeyLocation::Standard),
        110 => (KeyCode::F19, KeyLocation::Standard),
        111 => (KeyCode::F20, KeyLocation::Standard),
        112 => (KeyCode::F21, KeyLocation::Standard),
        113 => (KeyCode::F22, KeyLocation::Standard),
        114 => (KeyCode::F23, KeyLocation::Standard),
        115 => (KeyCode::F24, KeyLocation::Standard),
        224 => (KeyCode::ControlLeft, KeyLocation::Left),
        225 => (KeyCode::ShiftLeft, KeyLocation::Left),
        226 => (KeyCode::AltLeft, KeyLocation::Left),
        227 => (KeyCode::SuperLeft, KeyLocation::Left),
        228 => (KeyCode::ControlRight, KeyLocation::Right),
        229 => (KeyCode::ShiftRight, KeyLocation::Right),
        230 => (KeyCode::AltRight, KeyLocation::Right),
        231 => (KeyCode::SuperRight, KeyLocation::Right),
        _ => return None,
    })
}
