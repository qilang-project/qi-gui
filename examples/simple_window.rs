//! Simple window example
//!
//! Creates a basic window and runs the event loop.

use qi_gui::{Window, EventLoop};

fn main() {
    println!("=== Simple Window Example ===");
    println!("Creating window...");

    // Create event loop
    let event_loop = EventLoop::new();

    // Create window
    let window = Window::new(event_loop.inner(), "Simple Window - 简单窗口", 800, 600)
        .expect("Failed to create window");

    println!("Window created! Title: {}", window.title());

    // Show window
    window.show();

    println!("Running event loop. Close the window to exit.");

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
                println!("Window close requested");
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                println!("Window resized to {}x{}", size.width, size.height);
            }
            _ => {}
        }
    });

    println!("Event loop exited. Program ending.");
}
