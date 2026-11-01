use crate::*;

/// M49: the persistable ChordKey for an egui key (None for keys the
/// shortcut table has no slot for: Escape, Enter, modifier keys, F13+).
pub(crate) fn chord_key_from_egui(k: egui::Key) -> Option<ChordKey> {
    Some(match k {
        egui::Key::A => ChordKey::A,
        egui::Key::B => ChordKey::B,
        egui::Key::C => ChordKey::C,
        egui::Key::D => ChordKey::D,
        egui::Key::E => ChordKey::E,
        egui::Key::F => ChordKey::F,
        egui::Key::G => ChordKey::G,
        egui::Key::H => ChordKey::H,
        egui::Key::I => ChordKey::I,
        egui::Key::J => ChordKey::J,
        egui::Key::K => ChordKey::K,
        egui::Key::L => ChordKey::L,
        egui::Key::M => ChordKey::M,
        egui::Key::N => ChordKey::N,
        egui::Key::O => ChordKey::O,
        egui::Key::P => ChordKey::P,
        egui::Key::Q => ChordKey::Q,
        egui::Key::R => ChordKey::R,
        egui::Key::S => ChordKey::S,
        egui::Key::T => ChordKey::T,
        egui::Key::U => ChordKey::U,
        egui::Key::V => ChordKey::V,
        egui::Key::W => ChordKey::W,
        egui::Key::X => ChordKey::X,
        egui::Key::Y => ChordKey::Y,
        egui::Key::Z => ChordKey::Z,
        egui::Key::Num0 => ChordKey::Num0,
        egui::Key::Num1 => ChordKey::Num1,
        egui::Key::Num2 => ChordKey::Num2,
        egui::Key::Num3 => ChordKey::Num3,
        egui::Key::Num4 => ChordKey::Num4,
        egui::Key::Num5 => ChordKey::Num5,
        egui::Key::Num6 => ChordKey::Num6,
        egui::Key::Num7 => ChordKey::Num7,
        egui::Key::Num8 => ChordKey::Num8,
        egui::Key::Num9 => ChordKey::Num9,
        egui::Key::F1 => ChordKey::F1,
        egui::Key::F2 => ChordKey::F2,
        egui::Key::F3 => ChordKey::F3,
        egui::Key::F4 => ChordKey::F4,
        egui::Key::F5 => ChordKey::F5,
        egui::Key::F6 => ChordKey::F6,
        egui::Key::F7 => ChordKey::F7,
        egui::Key::F8 => ChordKey::F8,
        egui::Key::F9 => ChordKey::F9,
        egui::Key::F10 => ChordKey::F10,
        egui::Key::F11 => ChordKey::F11,
        egui::Key::F12 => ChordKey::F12,
        egui::Key::ArrowUp => ChordKey::ArrowUp,
        egui::Key::ArrowDown => ChordKey::ArrowDown,
        egui::Key::ArrowLeft => ChordKey::ArrowLeft,
        egui::Key::ArrowRight => ChordKey::ArrowRight,
        egui::Key::Home => ChordKey::Home,
        egui::Key::End => ChordKey::End,
        egui::Key::PageUp => ChordKey::PageUp,
        egui::Key::PageDown => ChordKey::PageDown,
        egui::Key::Insert => ChordKey::Insert,
        egui::Key::Space => ChordKey::Space,
        egui::Key::Tab => ChordKey::Tab,
        egui::Key::Comma => ChordKey::Comma,
        egui::Key::Period => ChordKey::Period,
        egui::Key::Slash => ChordKey::Slash,
        egui::Key::Semicolon => ChordKey::Semicolon,
        egui::Key::Quote => ChordKey::Quote,
        egui::Key::Minus => ChordKey::Minus,
        egui::Key::Equals => ChordKey::Equals,
        egui::Key::OpenBracket => ChordKey::OpenBracket,
        egui::Key::CloseBracket => ChordKey::CloseBracket,
        egui::Key::Backslash => ChordKey::Backslash,
        egui::Key::Backtick => ChordKey::Backtick,
        _ => return None,
    })
}

/// M49: this event as a chord, when it is a key press (repeats included,
/// matching the old `consume_key` handling).
pub(crate) fn chord_from_event(ev: &egui::Event) -> Option<KeyChord> {
    let egui::Event::Key {
        key,
        pressed: true,
        modifiers,
        ..
    } = ev
    else {
        return None;
    };
    Some(KeyChord::new(
        chord_key_from_egui(*key)?,
        modifiers.ctrl,
        modifiers.shift,
        modifiers.alt,
    ))
}

/// M49: consume the first key event whose exact chord the table owns and
/// return that action (the event leaves the queue so nothing below
/// re-reads it). The `while` loop at the call site terminates: every hit
/// removes one event.
pub(crate) fn take_shortcut_action(shortcuts: &Shortcuts, ctx: &egui::Context) -> Option<ActionId> {
    ctx.input_mut(|i| {
        let pos = i.events.iter().position(|ev| {
            chord_from_event(ev)
                .and_then(|c| shortcuts.owner_of(c))
                .is_some()
        })?;
        let ev = i.events.remove(pos);
        shortcuts.owner_of(chord_from_event(&ev)?)
    })
}
