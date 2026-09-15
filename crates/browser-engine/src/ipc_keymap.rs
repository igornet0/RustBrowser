//! Map IPC key strings to Servo `Key` / `NamedKey` (Content Process).

use servo::{Key, NamedKey};

/// Parse `InputEventMsg::Key.key` produced by Browser (`Character` or `Named:…`).
pub fn parse_ipc_key(key: &str) -> Key {
    if let Some(named) = key.strip_prefix("Named:") {
        return Key::Named(parse_named_key(named.trim()));
    }
    if key == " " || key.eq_ignore_ascii_case("space") {
        return Key::Character(" ".to_string());
    }
    Key::Character(key.to_string())
}

pub fn parse_named_key(named: &str) -> NamedKey {
    // Debug format from winit NamedKey is often `Enter`, sometimes with extras.
    let name = named
        .strip_prefix("NamedKey::")
        .unwrap_or(named)
        .split(|c: char| c == '(' || c == ' ')
        .next()
        .unwrap_or(named);

    match name {
        "Enter" => NamedKey::Enter,
        "Tab" => NamedKey::Tab,
        "Backspace" => NamedKey::Backspace,
        "Delete" => NamedKey::Delete,
        "Escape" => NamedKey::Escape,
        "ArrowLeft" => NamedKey::ArrowLeft,
        "ArrowRight" => NamedKey::ArrowRight,
        "ArrowUp" => NamedKey::ArrowUp,
        "ArrowDown" => NamedKey::ArrowDown,
        "Home" => NamedKey::Home,
        "End" => NamedKey::End,
        "PageUp" => NamedKey::PageUp,
        "PageDown" => NamedKey::PageDown,
        "Insert" => NamedKey::Insert,
        "F1" => NamedKey::F1,
        "F2" => NamedKey::F2,
        "F3" => NamedKey::F3,
        "F4" => NamedKey::F4,
        "F5" => NamedKey::F5,
        "F6" => NamedKey::F6,
        "F7" => NamedKey::F7,
        "F8" => NamedKey::F8,
        "F9" => NamedKey::F9,
        "F10" => NamedKey::F10,
        "F11" => NamedKey::F11,
        "F12" => NamedKey::F12,
        "Meta" | "Super" => NamedKey::Meta,
        "Control" => NamedKey::Control,
        "Alt" => NamedKey::Alt,
        "Shift" => NamedKey::Shift,
        "CapsLock" => NamedKey::CapsLock,
        "ContextMenu" => NamedKey::ContextMenu,
        _ => NamedKey::Unidentified,
    }
}

/// Modifier bitmask shared with Browser IPC (`InputEventMsg::Key.modifiers`).
pub mod mods {
    pub const SHIFT: u8 = 0x01;
    pub const CONTROL: u8 = 0x02;
    pub const ALT: u8 = 0x04;
    pub const META: u8 = 0x08; // Cmd on macOS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_and_digits() {
        assert!(matches!(parse_ipc_key("a"), Key::Character(s) if s == "a"));
        assert!(matches!(parse_ipc_key("Z"), Key::Character(s) if s == "Z"));
        assert!(matches!(parse_ipc_key("5"), Key::Character(s) if s == "5"));
    }

    #[test]
    fn named_navigation_keys() {
        assert!(matches!(
            parse_ipc_key("Named:Enter"),
            Key::Named(NamedKey::Enter)
        ));
        assert!(matches!(
            parse_ipc_key("Named:Escape"),
            Key::Named(NamedKey::Escape)
        ));
        assert!(matches!(
            parse_ipc_key("Named:Tab"),
            Key::Named(NamedKey::Tab)
        ));
        assert!(matches!(
            parse_ipc_key("Named:Backspace"),
            Key::Named(NamedKey::Backspace)
        ));
        assert!(matches!(
            parse_ipc_key("Named:Delete"),
            Key::Named(NamedKey::Delete)
        ));
        assert!(matches!(
            parse_ipc_key("Named:ArrowLeft"),
            Key::Named(NamedKey::ArrowLeft)
        ));
        assert!(matches!(
            parse_ipc_key("Named:Home"),
            Key::Named(NamedKey::Home)
        ));
        assert!(matches!(
            parse_ipc_key("Named:PageDown"),
            Key::Named(NamedKey::PageDown)
        ));
    }

    #[test]
    fn function_keys() {
        assert!(matches!(parse_ipc_key("Named:F5"), Key::Named(NamedKey::F5)));
        assert!(matches!(
            parse_ipc_key("Named:F12"),
            Key::Named(NamedKey::F12)
        ));
    }

    #[test]
    fn modifiers_named() {
        assert!(matches!(
            parse_ipc_key("Named:Meta"),
            Key::Named(NamedKey::Meta)
        ));
        assert!(matches!(
            parse_ipc_key("Named:Control"),
            Key::Named(NamedKey::Control)
        ));
        assert!(matches!(
            parse_ipc_key("Named:Alt"),
            Key::Named(NamedKey::Alt)
        ));
        assert!(matches!(
            parse_ipc_key("Named:Shift"),
            Key::Named(NamedKey::Shift)
        ));
    }

    #[test]
    fn space() {
        assert!(matches!(parse_ipc_key(" "), Key::Character(s) if s == " "));
        assert!(matches!(parse_ipc_key("Space"), Key::Character(s) if s == " "));
    }
}
