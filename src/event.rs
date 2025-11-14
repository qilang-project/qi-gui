//! Event loop management module

use tao::event_loop::{EventLoop as TaoEventLoop, EventLoopWindowTarget, ControlFlow};
use tao::event::{Event, WindowEvent};
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

/// Event loop wrapper
pub struct EventLoop {
    inner: TaoEventLoop<()>,
    callbacks: Arc<Mutex<EventCallbacks>>,
}

/// Event callbacks storage
struct EventCallbacks {
    on_close: HashMap<u64, Box<dyn FnMut() + Send>>,
}

impl EventCallbacks {
    fn new() -> Self {
        EventCallbacks {
            on_close: HashMap::new(),
        }
    }
}

impl EventLoop {
    /// Create a new event loop
    pub fn new() -> Self {
        EventLoop {
            inner: TaoEventLoop::new(),
            callbacks: Arc::new(Mutex::new(EventCallbacks::new())),
        }
    }

    /// Get reference to inner Tao event loop
    pub fn inner(&self) -> &TaoEventLoop<()> {
        &self.inner
    }

    /// Register close callback for a window
    pub fn on_close<F>(&mut self, window_id: u64, callback: F)
    where
        F: FnMut() + Send + 'static,
    {
        self.callbacks
            .lock()
            .unwrap()
            .on_close
            .insert(window_id, Box::new(callback));
    }

    /// Run the event loop
    pub fn run<F>(self, mut event_handler: F)
    where
        F: FnMut(Event<'_, ()>, &EventLoopWindowTarget<()>, &mut ControlFlow) + 'static,
    {
        let callbacks = self.callbacks.clone();

        self.inner.run(move |event, event_loop, control_flow| {
            *control_flow = ControlFlow::Wait;

            // Handle window close events
            if let Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                window_id,
                ..
            } = &event
            {
                let window_id_u64 = unsafe { std::mem::transmute::<tao::window::WindowId, u64>(*window_id) };
                if let Some(callback) = callbacks.lock().unwrap().on_close.get_mut(&window_id_u64) {
                    callback();
                }
                *control_flow = ControlFlow::Exit;
            }

            // Call user event handler
            event_handler(event, event_loop, control_flow);
        });
    }
}

impl Default for EventLoop {
    fn default() -> Self {
        Self::new()
    }
}
