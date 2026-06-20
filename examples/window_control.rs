//! Window control demonstration
//!
//! Shows how to control window position, size, and other properties.

use qi_gui::{EventLoop, Window};
use std::thread;
use std::time::Duration;

fn main() {
    println!("=== Window Control Demo ===");
    println!();

    let event_loop = EventLoop::new();
    let window = Window::new(event_loop.inner(), "Window Control - 窗口控制", 800, 600)
        .expect("Failed to create window");

    window.show();

    println!("Initial window properties:");
    let (width, height) = window.size();
    let (x, y) = window.position();
    println!("  Position: ({}, {})", x, y);
    println!("  Size: {}x{}", width, height);
    println!("  Title: {}", window.title());
    println!();

    // Demonstrate window control in a separate thread
    let window_clone = window.clone();
    std::thread::spawn(move || {
        thread::sleep(Duration::from_secs(2));

        println!("Moving window to (100, 100)...");
        window_clone.set_position(100, 100);
        thread::sleep(Duration::from_secs(2));

        println!("Resizing window to 1024x768...");
        window_clone.set_size(1024, 768);
        thread::sleep(Duration::from_secs(2));

        println!("Changing title...");
        window_clone.set_title("Modified Title - 修改后的标题");
        thread::sleep(Duration::from_secs(2));

        println!("Moving to center of screen (approximately)...");
        window_clone.set_position(200, 150);
        window_clone.set_size(1200, 900);
        thread::sleep(Duration::from_secs(2));

        println!("Restoring original size...");
        window_clone.set_size(800, 600);
        window_clone.set_title("Window Control - 窗口控制");

        println!();
        println!("Demo complete! Close the window to exit.");
    });

    // Run event loop
    event_loop.run(|event, _event_loop, control_flow| {
        use tao::event::{Event, WindowEvent};
        use tao::event_loop::ControlFlow;

        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                println!("Window closed");
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                println!("  → Window resized to {}x{}", size.width, size.height);
            }
            Event::WindowEvent {
                event: WindowEvent::Moved(pos),
                ..
            } => {
                println!("  → Window moved to ({}, {})", pos.x, pos.y);
            }
            _ => {}
        }
    });
}
