/// Keyboard key code mapping module
/// Maps Tao's keyboard keys to numeric codes for Qi language

use tao::keyboard::Key;

/// Map Tao keyboard key to numeric keycode
/// Returns the keycode as i64
pub fn map_key_to_code(key: &Key) -> i64 {
    match key {
        // Character keys (printable characters)
        Key::Character(s) => {
            if let Some(ch) = s.chars().next() {
                ch as i64
            } else {
                0
            }
        }

        // Function keys (F1-F35)
        Key::F1 => 0x70,
        Key::F2 => 0x71,
        Key::F3 => 0x72,
        Key::F4 => 0x73,
        Key::F5 => 0x74,
        Key::F6 => 0x75,
        Key::F7 => 0x76,
        Key::F8 => 0x77,
        Key::F9 => 0x78,
        Key::F10 => 0x79,
        Key::F11 => 0x7A,
        Key::F12 => 0x7B,
        Key::F13 => 0x7C,
        Key::F14 => 0x7D,
        Key::F15 => 0x7E,
        Key::F16 => 0x7F,
        Key::F17 => 0x80,
        Key::F18 => 0x81,
        Key::F19 => 0x82,
        Key::F20 => 0x83,
        Key::F21 => 0x84,
        Key::F22 => 0x85,
        Key::F23 => 0x86,
        Key::F24 => 0x87,
        Key::F25 => 0x88,
        Key::F26 => 0x89,
        Key::F27 => 0x8A,
        Key::F28 => 0x8B,
        Key::F29 => 0x8C,
        Key::F30 => 0x8D,
        Key::F31 => 0x8E,
        Key::F32 => 0x8F,
        Key::F33 => 0x90,
        Key::F34 => 0x91,
        Key::F35 => 0x92,

        // Arrow keys
        Key::ArrowDown => 0x28,  // Down arrow
        Key::ArrowLeft => 0x25,  // Left arrow
        Key::ArrowRight => 0x27, // Right arrow
        Key::ArrowUp => 0x26,    // Up arrow

        // Editing keys
        Key::Backspace => 0x08,  // ASCII backspace
        Key::Delete => 0x2E,     // Delete
        Key::End => 0x23,        // End
        Key::Enter => 0x0D,      // ASCII carriage return
        Key::Escape => 0x1B,     // ASCII escape
        Key::Home => 0x24,       // Home
        Key::Insert => 0x2D,     // Insert
        Key::PageDown => 0x22,   // Page Down
        Key::PageUp => 0x21,     // Page Up
        Key::Tab => 0x09,        // ASCII tab
        Key::Space => 0x20,      // ASCII space

        // Modifier keys
        Key::Alt => 0x12,        // Alt
        Key::AltGraph => 0x12,   // Alt Graph (treat as Alt)
        Key::CapsLock => 0x14,   // Caps Lock
        Key::Control => 0x11,    // Control
        Key::Super => 0x5B,      // Meta/Command/Windows key
        Key::Shift => 0x10,      // Shift

        // Lock keys
        Key::NumLock => 0x90,    // Num Lock
        Key::ScrollLock => 0x91, // Scroll Lock

        // Media keys
        Key::AudioVolumeDown => 0xAE,
        Key::AudioVolumeMute => 0xAD,
        Key::AudioVolumeUp => 0xAF,
        Key::MediaPlayPause => 0xB3,
        Key::MediaStop => 0xB2,
        Key::MediaTrackNext => 0xB0,
        Key::MediaTrackPrevious => 0xB1,

        // Browser keys
        Key::BrowserBack => 0xA6,
        Key::BrowserFavorites => 0xAB,
        Key::BrowserForward => 0xA7,
        Key::BrowserHome => 0xAC,
        Key::BrowserRefresh => 0xA8,
        Key::BrowserSearch => 0xAA,
        Key::BrowserStop => 0xA9,

        // Other named keys
        Key::Clear => 0x0C,      // Clear
        Key::ContextMenu => 0x5D, // Context menu
        Key::Pause => 0x13,      // Pause
        Key::PrintScreen => 0x2C, // Print Screen
        Key::Select => 0x29,     // Select

        // Default for unhandled keys (Unidentified, Dead, etc.)
        _ => 0,
    }
}

/// Get modifier key state as a bitmask
/// Bit 0: Shift
/// Bit 1: Control
/// Bit 2: Alt
/// Bit 3: Meta/Command/Windows
pub fn get_modifier_mask(modifiers: &tao::keyboard::ModifiersState) -> i64 {
    let mut mask = 0i64;

    if modifiers.shift_key() {
        mask |= 1 << 0; // Bit 0
    }
    if modifiers.control_key() {
        mask |= 1 << 1; // Bit 1
    }
    if modifiers.alt_key() {
        mask |= 1 << 2; // Bit 2
    }
    if modifiers.super_key() {
        mask |= 1 << 3; // Bit 3 (super = Meta/Command/Windows)
    }

    mask
}

/// Check if a specific modifier is active
pub fn has_shift(modifiers: &tao::keyboard::ModifiersState) -> bool {
    modifiers.shift_key()
}

pub fn has_control(modifiers: &tao::keyboard::ModifiersState) -> bool {
    modifiers.control_key()
}

pub fn has_alt(modifiers: &tao::keyboard::ModifiersState) -> bool {
    modifiers.alt_key()
}

pub fn has_meta(modifiers: &tao::keyboard::ModifiersState) -> bool {
    modifiers.super_key()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_function_keys() {
        assert_eq!(map_key_to_code(&Key::F1), 0x70);
        assert_eq!(map_key_to_code(&Key::F12), 0x7B);
        assert_eq!(map_key_to_code(&Key::F24), 0x87);
    }

    #[test]
    fn test_special_keys() {
        assert_eq!(map_key_to_code(&Key::Enter), 0x0D);
        assert_eq!(map_key_to_code(&Key::Escape), 0x1B);
        assert_eq!(map_key_to_code(&Key::ArrowUp), 0x26);
        assert_eq!(map_key_to_code(&Key::Space), 0x20);
    }

    #[test]
    fn test_character_keys() {
        let key = Key::Character("a".into());
        assert_eq!(map_key_to_code(&key), 'a' as i64);

        let key = Key::Character("A".into());
        assert_eq!(map_key_to_code(&key), 'A' as i64);

        let key = Key::Character("1".into());
        assert_eq!(map_key_to_code(&key), '1' as i64);
    }
}
