//! Keyboard event demonstration
//!
//! Shows all keyboard events including character keys, function keys,
//! and modifier key combinations.

use qi_gui::{Window, EventLoop};

fn main() {
    println!("=== Keyboard Event Demo ===");
    println!("Press keys to see keyboard events");
    println!("Try:");
    println!("  - Character keys (a-z, A-Z, 0-9)");
    println!("  - Function keys (F1-F12)");
    println!("  - Special keys (Enter, Escape, Arrow keys)");
    println!("  - Modifier combinations (Ctrl+C, Shift+A, etc.)");
    println!();

    let event_loop = EventLoop::new();
    let window = Window::new(event_loop.inner(), "Keyboard Demo - 键盘演示", 800, 600)
        .expect("Failed to create window");

    window.show();

    // Track modifier state
    let mut modifiers = tao::keyboard::ModifiersState::empty();

    event_loop.run(move |event, _event_loop, control_flow| {
        use tao::event::{Event, WindowEvent};
        use tao::event_loop::ControlFlow;
        use qi_gui::keycode;

        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                println!("Closing window...");
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::ModifiersChanged(new_modifiers),
                ..
            } => {
                modifiers = new_modifiers;
            }
            Event::WindowEvent {
                event: WindowEvent::KeyboardInput { event: key_event, .. },
                ..
            } => {
                // Get keycode
                let keycode = keycode::map_key_to_code(&key_event.logical_key);
                let modifier_mask = keycode::get_modifier_mask(&modifiers);

                // Format key name
                let key_name = if keycode >= 0x20 && keycode < 0x7F {
                    format!("'{}'", char::from_u32(keycode as u32).unwrap_or('?'))
                } else {
                    format!("0x{:02X}", keycode)
                };

                // Format modifier keys
                let mut mods = Vec::new();
                if modifier_mask & (1 << 0) != 0 { mods.push("Shift"); }
                if modifier_mask & (1 << 1) != 0 { mods.push("Ctrl"); }
                if modifier_mask & (1 << 2) != 0 { mods.push("Alt"); }
                if modifier_mask & (1 << 3) != 0 { mods.push("Cmd"); }

                if mods.is_empty() {
                    println!("Key pressed: {}", key_name);
                } else {
                    println!("Key pressed: {} + {}", mods.join("+"), key_name);
                }
            }
            _ => {}
        }
    });
}
