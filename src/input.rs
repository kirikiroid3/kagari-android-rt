//! winit key events → engine [`InputEvent`]s: keyboard keys, the back key before API 33, and
//! gamepad buttons.

use std::collections::{HashMap, HashSet};

use kagari_engine::{InputEvent, Key, PadButton};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{KeyCode, NativeKeyCode, PhysicalKey};

/// Android's `KEYCODE_BACK`.
const KEYCODE_BACK: u32 = 4;

/// The first API level whose back gesture reaches `GameActivity.nativeOnBack` instead of a key
/// event.
const BACK_CALLBACK_SDK: i32 = 33;

/// The engine key of a physical key code.
pub fn map_key(code: KeyCode) -> Option<Key> {
    Some(match code {
        KeyCode::Enter | KeyCode::NumpadEnter => Key::Enter,
        KeyCode::Escape => Key::Escape,
        KeyCode::Space => Key::Space,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Tab => Key::Tab,
        KeyCode::ArrowUp => Key::Up,
        KeyCode::ArrowDown => Key::Down,
        KeyCode::ArrowLeft => Key::Left,
        KeyCode::ArrowRight => Key::Right,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::ShiftLeft | KeyCode::ShiftRight => Key::Shift,
        KeyCode::ControlLeft | KeyCode::ControlRight => Key::Control,
        KeyCode::AltLeft | KeyCode::AltRight => Key::Alt,
        KeyCode::F1 => Key::F1,
        KeyCode::F2 => Key::F2,
        KeyCode::F3 => Key::F3,
        KeyCode::F4 => Key::F4,
        KeyCode::F5 => Key::F5,
        KeyCode::F6 => Key::F6,
        KeyCode::F7 => Key::F7,
        KeyCode::F8 => Key::F8,
        KeyCode::F9 => Key::F9,
        KeyCode::F10 => Key::F10,
        KeyCode::F11 => Key::F11,
        KeyCode::F12 => Key::F12,
        KeyCode::KeyA => Key::A,
        KeyCode::KeyB => Key::B,
        KeyCode::KeyC => Key::C,
        KeyCode::KeyD => Key::D,
        KeyCode::KeyE => Key::E,
        KeyCode::KeyF => Key::F,
        KeyCode::KeyG => Key::G,
        KeyCode::KeyH => Key::H,
        KeyCode::KeyI => Key::I,
        KeyCode::KeyJ => Key::J,
        KeyCode::KeyK => Key::K,
        KeyCode::KeyL => Key::L,
        KeyCode::KeyM => Key::M,
        KeyCode::KeyN => Key::N,
        KeyCode::KeyO => Key::O,
        KeyCode::KeyP => Key::P,
        KeyCode::KeyQ => Key::Q,
        KeyCode::KeyR => Key::R,
        KeyCode::KeyS => Key::S,
        KeyCode::KeyT => Key::T,
        KeyCode::KeyU => Key::U,
        KeyCode::KeyV => Key::V,
        KeyCode::KeyW => Key::W,
        KeyCode::KeyX => Key::X,
        KeyCode::KeyY => Key::Y,
        KeyCode::KeyZ => Key::Z,
        KeyCode::Digit0 => Key::Digit0,
        KeyCode::Digit1 => Key::Digit1,
        KeyCode::Digit2 => Key::Digit2,
        KeyCode::Digit3 => Key::Digit3,
        KeyCode::Digit4 => Key::Digit4,
        KeyCode::Digit5 => Key::Digit5,
        KeyCode::Digit6 => Key::Digit6,
        KeyCode::Digit7 => Key::Digit7,
        KeyCode::Digit8 => Key::Digit8,
        KeyCode::Digit9 => Key::Digit9,
        _ => return None,
    })
}

/// The engine button of an Android gamepad keycode (`KEYCODE_BUTTON_*`). D-pads arrive as arrow
/// keys: winit maps `KEYCODE_DPAD_*` to them.
fn map_pad(code: u32) -> Option<PadButton> {
    Some(match code {
        96 => PadButton::South,
        97 => PadButton::East,
        99 => PadButton::West,
        100 => PadButton::North,
        102 => PadButton::LeftShoulder,
        103 => PadButton::RightShoulder,
        104 => PadButton::LeftTrigger,
        105 => PadButton::RightTrigger,
        106 => PadButton::LeftStick,
        107 => PadButton::RightStick,
        108 => PadButton::Start,
        109 => PadButton::Select,
        _ => return None,
    })
}

/// A key that is down: a mapped key code, or an Android keycode winit leaves unidentified.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Held {
    Code(KeyCode),
    Android(u32),
}

/// The keys held down, so an engine key is pressed when its first key goes down and released
/// when its last key comes up (both Shift keys are one `Key::Shift`).
#[derive(Default)]
pub struct HeldKeys {
    held: HashSet<Held>,
    counts: HashMap<Key, u32>,
}

impl HeldKeys {
    /// The engine event of a winit key event on API level `sdk`, if it changes an engine key's
    /// or pad button's state. Repeats and unmapped keys give `None`.
    pub fn key(&mut self, event: &KeyEvent, sdk: i32) -> Option<InputEvent> {
        if event.repeat {
            return None;
        }
        let pressed = event.state == ElementState::Pressed;
        match event.physical_key {
            PhysicalKey::Code(code) => {
                let key = map_key(code)?;
                self.engine_key(Held::Code(code), key, pressed)
            }
            PhysicalKey::Unidentified(NativeKeyCode::Android(KEYCODE_BACK))
                if sdk < BACK_CALLBACK_SDK =>
            {
                self.engine_key(Held::Android(KEYCODE_BACK), Key::Escape, pressed)
            }
            PhysicalKey::Unidentified(NativeKeyCode::Android(code)) => {
                let button = map_pad(code)?;
                self.toggle(Held::Android(code), pressed)
                    .then_some(InputEvent::Pad { button, pressed })
            }
            _ => None,
        }
    }

    /// Records `held` going down or up; false when it already was.
    fn toggle(&mut self, held: Held, pressed: bool) -> bool {
        if pressed {
            self.held.insert(held)
        } else {
            self.held.remove(&held)
        }
    }

    fn engine_key(&mut self, held: Held, key: Key, pressed: bool) -> Option<InputEvent> {
        if !self.toggle(held, pressed) {
            return None;
        }
        let count = self.counts.entry(key).or_insert(0);
        if pressed {
            *count = count.saturating_add(1);
            (*count == 1).then_some(InputEvent::Key { key, pressed: true })
        } else {
            *count = count.saturating_sub(1);
            (*count == 0).then_some(InputEvent::Key {
                key,
                pressed: false,
            })
        }
    }

    /// Focus lost: every held key is forgotten (the engine releases them on `Focus(false)`).
    pub fn clear(&mut self) {
        self.held.clear();
        self.counts.clear();
    }
}
