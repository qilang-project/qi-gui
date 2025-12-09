//! C FFI interface for Qi language

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::Mutex;
use std::collections::HashMap;
use tao::event_loop::{EventLoop as TaoEventLoop, ControlFlow};
use tao::event::Event;
use tao::window::WindowId;
use crate::window::Window;
use crate::keycode;
use crate::audio::AudioPlayer;

/// Event callback function type
/// Parameters: window_id, event_type, param1, param2
/// event_type:
///   0=CloseRequested
///   1=Resized(width, height)
///   2=KeyPressed(keycode, modifiers)
///       - param1 (keycode): Key code (character codes for A-Z, 0-9 or special key codes)
///       - param2 (modifiers): Bitmask - Bit0:Shift, Bit1:Ctrl, Bit2:Alt, Bit3:Meta/Command
///   3=MouseClicked(button, state)
///   4=MouseMoved(x, y)
///   5=MouseWheel(delta_x, delta_y)
type EventCallback = extern "C" fn(u64, i32, i64, i64);

/// Window creation request
struct WindowRequest {
    id: u64,
    title: String,
    width: u32,
    height: u32,
}

/// Global state for lazy initialization
struct GuiState {
    next_window_id: u64,
    pending_windows: Vec<WindowRequest>,
    window_titles: HashMap<u64, String>,
    event_callbacks: HashMap<u64, EventCallback>,
    window_id_map: HashMap<WindowId, u64>,
    created_windows: HashMap<u64, Window>,
    current_modifiers: tao::keyboard::ModifiersState,
    next_audio_id: u64,
}

impl GuiState {
    fn new() -> Self {
        GuiState {
            next_window_id: 1,
            pending_windows: Vec::new(),
            window_titles: HashMap::new(),
            event_callbacks: HashMap::new(),
            window_id_map: HashMap::new(),
            created_windows: HashMap::new(),
            current_modifiers: tao::keyboard::ModifiersState::empty(),
            next_audio_id: 1,
        }
    }
}

static GUI_STATE: Mutex<Option<GuiState>> = Mutex::new(None);

// Audio players stored separately (not Send/Sync safe)
use std::cell::RefCell;
use crate::renderer::Renderer;

thread_local! {
    static AUDIO_PLAYERS: RefCell<HashMap<u64, AudioPlayer>> = RefCell::new(HashMap::new());
    static RENDERERS: RefCell<HashMap<u64, Renderer>> = RefCell::new(HashMap::new());
}

fn get_gui_state() -> std::sync::MutexGuard<'static, Option<GuiState>> {
    let mut state = GUI_STATE.lock().unwrap();
    if state.is_none() {
        *state = Some(GuiState::new());
    }
    state
}

/// Create a window (queued until run is called)
/// Returns a window ID (non-zero on success, 0 on failure)
#[no_mangle]
pub extern "C" fn qi_gui_create_window_impl(
    title: *const c_char,
    width: u32,
    height: u32,
) -> u64 {
    if title.is_null() {
        return 0;
    }

    let title_str = unsafe {
        match CStr::from_ptr(title).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return 0,
        }
    };

    let mut state = get_gui_state();
    let state = state.as_mut().unwrap();

    let window_id = state.next_window_id;
    state.next_window_id += 1;

    state.pending_windows.push(WindowRequest {
        id: window_id,
        title: title_str.clone(),
        width,
        height,
    });

    state.window_titles.insert(window_id, title_str);

    window_id
}

/// Destroy a window (currently a no-op in lazy mode)
#[no_mangle]
pub extern "C" fn qi_gui_destroy_window_impl(window_id: u64) {
    if window_id == 0 {
        return;
    }

    let mut state = get_gui_state();
    if let Some(state) = state.as_mut() {
        state.window_titles.remove(&window_id);
        state.pending_windows.retain(|w| w.id != window_id);
    }
}

/// Set window title (updates queued window or title map)
#[no_mangle]
pub extern "C" fn qi_gui_set_title_impl(window_id: u64, title: *const c_char) {
    if window_id == 0 || title.is_null() {
        return;
    }

    let title_str = unsafe {
        match CStr::from_ptr(title).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return,
        }
    };

    let mut state = get_gui_state();
    if let Some(state) = state.as_mut() {
        state.window_titles.insert(window_id, title_str.clone());
        // Update pending window if it exists
        if let Some(window) = state.pending_windows.iter_mut().find(|w| w.id == window_id) {
            window.title = title_str;
        }
    }
}

/// Get window title
#[no_mangle]
pub extern "C" fn qi_gui_get_title_impl(window_id: u64) -> *mut c_char {
    if window_id == 0 {
        return std::ptr::null_mut();
    }

    let state = get_gui_state();
    if let Some(state) = state.as_ref() {
        if let Some(title) = state.window_titles.get(&window_id) {
            match CString::new(title.as_str()) {
                Ok(c_str) => return c_str.into_raw(),
                Err(_) => return std::ptr::null_mut(),
            }
        }
    }

    std::ptr::null_mut()
}

/// Show window (no-op in current implementation)
#[no_mangle]
pub extern "C" fn qi_gui_show_window_impl(_window_id: u64) {
    // Windows are shown by default when created in run()
}

/// Hide window (no-op in current implementation)
#[no_mangle]
pub extern "C" fn qi_gui_hide_window_impl(_window_id: u64) {
    // Not implemented yet
}

/// Check if window is visible
#[no_mangle]
pub extern "C" fn qi_gui_is_visible_impl(window_id: u64) -> i32 {
    if window_id == 0 {
        return 0;
    }

    let state = get_gui_state();
    if let Some(state) = state.as_ref() {
        if state.window_titles.contains_key(&window_id) {
            return 1;
        }
    }

    0
}

/// Default event callback that prints events to console
extern "C" fn default_event_callback(window_id: u64, event_type: i32, param1: i64, param2: i64) {
    match event_type {
        0 => println!("[窗口 {}] 关闭事件", window_id),
        1 => println!("[窗口 {}] 大小改变事件: {}x{}", window_id, param1, param2),
        2 => {
            // param1 = keycode, param2 = modifier mask
            let keycode = param1;
            let modifiers = param2;

            // Format key name
            let key_name = if keycode >= 0x20 && keycode < 0x7F {
                // Printable ASCII character
                format!("'{}'", char::from_u32(keycode as u32).unwrap_or('?'))
            } else {
                // Special key code
                format!("0x{:02X}", keycode)
            };

            // Format modifier keys
            let mut mods = Vec::new();
            if modifiers & (1 << 0) != 0 { mods.push("Shift"); }
            if modifiers & (1 << 1) != 0 { mods.push("Ctrl"); }
            if modifiers & (1 << 2) != 0 { mods.push("Alt"); }
            if modifiers & (1 << 3) != 0 { mods.push("Cmd"); }

            if mods.is_empty() {
                println!("[窗口 {}] 键盘事件: {}", window_id, key_name);
            } else {
                println!("[窗口 {}] 键盘事件: {} + {}", window_id, mods.join("+"), key_name);
            }
        }
        3 => {
            let button_name = match param1 {
                1 => "左键",
                2 => "右键",
                3 => "中键",
                _ => "未知",
            };
            let state_name = if param2 == 1 { "按下" } else { "释放" };
            println!("[窗口 {}] 鼠标{}事件: {}", window_id, button_name, state_name);
        }
        4 => println!("[窗口 {}] 鼠标移动: x={}, y={}", window_id, param1, param2),
        5 => println!("[窗口 {}] 鼠标滚轮: dx={}, dy={}", window_id, param1, param2),
        _ => println!("[窗口 {}] 未知事件类型: {}", window_id, event_type),
    }
}

/// Set event callback for a window
/// callback signature: fn(window_id: u64, event_type: i32, param1: i64, param2: i64)
/// event_type: 0=CloseRequested, 1=Resized(width, height), 2=KeyPressed(keycode), 3=MouseClicked(x, y)
#[no_mangle]
pub extern "C" fn qi_gui_set_event_callback_impl(window_id: u64, callback: EventCallback) {
    if window_id == 0 {
        return;
    }

    let mut state = get_gui_state();
    if let Some(state) = state.as_mut() {
        state.event_callbacks.insert(window_id, callback);
    }
}

/// Enable default event printing for a window (prints events to console)
#[no_mangle]
pub extern "C" fn qi_gui_enable_event_printing_impl(window_id: u64) {
    qi_gui_set_event_callback_impl(window_id, default_event_callback);
}

/// Get window X position
#[no_mangle]
pub extern "C" fn qi_gui_get_position_x_impl(window_id: u64) -> i64 {
    if window_id == 0 {
        return 0;
    }

    let state = get_gui_state();
    if let Some(state) = state.as_ref() {
        if let Some(window) = state.created_windows.get(&window_id) {
            let (x, _) = window.position();
            return x as i64;
        }
    }

    0
}

/// Get window Y position
#[no_mangle]
pub extern "C" fn qi_gui_get_position_y_impl(window_id: u64) -> i64 {
    if window_id == 0 {
        return 0;
    }

    let state = get_gui_state();
    if let Some(state) = state.as_ref() {
        if let Some(window) = state.created_windows.get(&window_id) {
            let (_, y) = window.position();
            return y as i64;
        }
    }

    0
}

/// Set window position
#[no_mangle]
pub extern "C" fn qi_gui_set_position_impl(window_id: u64, x: i32, y: i32) {
    if window_id == 0 {
        return;
    }

    let state = get_gui_state();
    if let Some(state) = state.as_ref() {
        if let Some(window) = state.created_windows.get(&window_id) {
            window.set_position(x, y);
        }
    }
}

/// Get window width
#[no_mangle]
pub extern "C" fn qi_gui_get_width_impl(window_id: u64) -> i64 {
    if window_id == 0 {
        return 0;
    }

    let state = get_gui_state();
    if let Some(state) = state.as_ref() {
        if let Some(window) = state.created_windows.get(&window_id) {
            let (w, _) = window.size();
            return w as i64;
        }
    }

    0
}

/// Get window height
#[no_mangle]
pub extern "C" fn qi_gui_get_height_impl(window_id: u64) -> i64 {
    if window_id == 0 {
        return 0;
    }

    let state = get_gui_state();
    if let Some(state) = state.as_ref() {
        if let Some(window) = state.created_windows.get(&window_id) {
            let (_, h) = window.size();
            return h as i64;
        }
    }

    0
}

/// Set window size
#[no_mangle]
pub extern "C" fn qi_gui_set_size_impl(window_id: u64, width: u32, height: u32) {
    if window_id == 0 {
        return;
    }

    let state = get_gui_state();
    if let Some(state) = state.as_ref() {
        if let Some(window) = state.created_windows.get(&window_id) {
            window.set_size(width, height);
        }
    }
}

/// Run the event loop (creates all pending windows and starts event processing)
#[no_mangle]
pub extern "C" fn qi_gui_run_impl() {
    // Get pending window requests and callbacks
    let (pending_windows, callbacks) = {
        let mut state = get_gui_state();
        if let Some(state) = state.as_mut() {
            let windows = std::mem::take(&mut state.pending_windows);
            let callbacks = state.event_callbacks.clone();
            (windows, callbacks)
        } else {
            (Vec::new(), HashMap::new())
        }
    };

    if pending_windows.is_empty() {
        return;
    }

    // Create event loop
    let event_loop = TaoEventLoop::new();

    // Create all pending windows and build window ID mapping
    let mut windows = Vec::new();
    let mut window_id_map = HashMap::new();

    for request in pending_windows {
        match Window::new(&event_loop, &request.title, request.width, request.height) {
            Ok(window) => {
                // Make window visible
                window.show();

                // Map Tao WindowId to our u64 ID
                window_id_map.insert(window.id(), request.id);

                // Store window in global state for later access
                {
                    let mut state = get_gui_state();
                    if let Some(state) = state.as_mut() {
                        state.created_windows.insert(request.id, window.clone());
                    }
                }

                windows.push(window);
            }
            Err(e) => eprintln!("Failed to create window '{}': {}", request.title, e),
        }
    }

    // Store window_id_map in global state
    {
        let mut state = get_gui_state();
        if let Some(state) = state.as_mut() {
            state.window_id_map = window_id_map.clone();
        }
    }

    // Run event loop
    event_loop.run(move |event, _event_loop, control_flow| {
        *control_flow = ControlFlow::Wait;

        match event {
            Event::WindowEvent { window_id, event, .. } => {
                // Get our window ID from Tao's WindowId
                let our_window_id = window_id_map.get(&window_id).copied().unwrap_or(0);

                match event {
                    tao::event::WindowEvent::CloseRequested => {
                        // Call callback if registered (event_type=0)
                        if let Some(callback) = callbacks.get(&our_window_id) {
                            callback(our_window_id, 0, 0, 0);
                        }
                        *control_flow = ControlFlow::Exit;
                    }
                    tao::event::WindowEvent::Resized(size) => {
                        // Call callback if registered (event_type=1, width, height)
                        if let Some(callback) = callbacks.get(&our_window_id) {
                            callback(our_window_id, 1, size.width as i64, size.height as i64);
                        }
                    }
                    tao::event::WindowEvent::ModifiersChanged(new_modifiers) => {
                        // Update stored modifier state
                        let mut state = get_gui_state();
                        if let Some(state) = state.as_mut() {
                            state.current_modifiers = new_modifiers;
                        }
                    }
                    tao::event::WindowEvent::KeyboardInput { event, .. } => {
                        // Call callback if registered (event_type=2, keycode, modifiers)
                        if let Some(callback) = callbacks.get(&our_window_id) {
                            // Map key to keycode using our keycode module
                            let keycode = keycode::map_key_to_code(&event.logical_key);

                            // Get modifier state as bitmask from stored state
                            let modifiers = {
                                let state = get_gui_state();
                                if let Some(state) = state.as_ref() {
                                    keycode::get_modifier_mask(&state.current_modifiers)
                                } else {
                                    0
                                }
                            };

                            callback(our_window_id, 2, keycode, modifiers);
                        }
                    }
                    tao::event::WindowEvent::MouseInput { button, state, .. } => {
                        // Call callback if registered (event_type=3, button, state)
                        if let Some(callback) = callbacks.get(&our_window_id) {
                            let button_code = match button {
                                tao::event::MouseButton::Left => 1,
                                tao::event::MouseButton::Right => 2,
                                tao::event::MouseButton::Middle => 3,
                                _ => 0,
                            };
                            let state_code = match state {
                                tao::event::ElementState::Pressed => 1,
                                tao::event::ElementState::Released => 0,
                                _ => 0,
                            };
                            callback(our_window_id, 3, button_code, state_code);
                        }
                    }
                    tao::event::WindowEvent::CursorMoved { position, .. } => {
                        // Call callback if registered (event_type=4, x, y)
                        if let Some(callback) = callbacks.get(&our_window_id) {
                            callback(our_window_id, 4, position.x as i64, position.y as i64);
                        }
                    }
                    tao::event::WindowEvent::MouseWheel { delta, .. } => {
                        // Call callback if registered (event_type=5, delta_x, delta_y)
                        if let Some(callback) = callbacks.get(&our_window_id) {
                            let (dx, dy) = match delta {
                                tao::event::MouseScrollDelta::LineDelta(x, y) => {
                                    (x as i64, y as i64)
                                }
                                tao::event::MouseScrollDelta::PixelDelta(pos) => {
                                    (pos.x as i64, pos.y as i64)
                                }
                                _ => (0, 0),
                            };
                            callback(our_window_id, 5, dx, dy);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    });
}

/// Free a string allocated by this library
#[no_mangle]
pub extern "C" fn qi_gui_free_string_impl(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    unsafe {
        let _ = CString::from_raw(s);
    }
}

/// Get library version
#[no_mangle]
pub extern "C" fn qi_gui_version_impl() -> *mut c_char {
    let version = CString::new("qi-gui 0.1.0").unwrap();
    version.into_raw()
}

// ============================================================================
// Audio FFI Functions
// ============================================================================

/// Load an audio file and create a player
/// Returns audio player ID (> 0) on success, 0 on failure
/// Supports: MP3, WAV, FLAC, Vorbis
#[no_mangle]
pub extern "C" fn qi_gui_audio_load_impl(file_path: *const c_char) -> u64 {
    if file_path.is_null() {
        return 0;
    }

    let c_str = unsafe { CStr::from_ptr(file_path) };
    let path_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => return 0,
    };

    let player = match AudioPlayer::new(path_str) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Failed to load audio file {}: {}", path_str, e);
            return 0;
        }
    };

    let mut state = get_gui_state();
    if state.is_none() {
        *state = Some(GuiState::new());
    }

    if let Some(state) = state.as_mut() {
        let audio_id = state.next_audio_id;
        state.next_audio_id += 1;

        // Store player in thread-local storage
        AUDIO_PLAYERS.with(|players| {
            players.borrow_mut().insert(audio_id, player);
        });

        audio_id
    } else {
        0
    }
}

/// Play audio
#[no_mangle]
pub extern "C" fn qi_gui_audio_play_impl(audio_id: u64) {
    if audio_id == 0 {
        return;
    }

    AUDIO_PLAYERS.with(|players| {
        if let Some(player) = players.borrow().get(&audio_id) {
            player.play();
        }
    });
}

/// Pause audio
#[no_mangle]
pub extern "C" fn qi_gui_audio_pause_impl(audio_id: u64) {
    if audio_id == 0 {
        return;
    }

    AUDIO_PLAYERS.with(|players| {
        if let Some(player) = players.borrow().get(&audio_id) {
            player.pause();
        }
    });
}

/// Stop audio
#[no_mangle]
pub extern "C" fn qi_gui_audio_stop_impl(audio_id: u64) {
    if audio_id == 0 {
        return;
    }

    AUDIO_PLAYERS.with(|players| {
        if let Some(player) = players.borrow().get(&audio_id) {
            player.stop();
        }
    });
}

/// Set audio volume (0.0 to 1.0)
#[no_mangle]
pub extern "C" fn qi_gui_audio_set_volume_impl(audio_id: u64, volume: f32) {
    if audio_id == 0 {
        return;
    }

    AUDIO_PLAYERS.with(|players| {
        if let Some(player) = players.borrow().get(&audio_id) {
            player.set_volume(volume);
        }
    });
}

/// Check if audio is playing (returns 1 if playing, 0 if not)
#[no_mangle]
pub extern "C" fn qi_gui_audio_is_playing_impl(audio_id: u64) -> i32 {
    if audio_id == 0 {
        return 0;
    }

    AUDIO_PLAYERS.with(|players| {
        if let Some(player) = players.borrow().get(&audio_id) {
            if player.is_playing() { 1 } else { 0 }
        } else {
            0
        }
    })
}

/// Check if audio is finished (returns 1 if finished, 0 if not)
#[no_mangle]
pub extern "C" fn qi_gui_audio_is_finished_impl(audio_id: u64) -> i32 {
    if audio_id == 0 {
        return 0;
    }

    AUDIO_PLAYERS.with(|players| {
        if let Some(player) = players.borrow().get(&audio_id) {
            if player.is_finished() { 1 } else { 0 }
        } else {
            0
        }
    })
}

/// Free/release an audio player
#[no_mangle]
pub extern "C" fn qi_gui_audio_free_impl(audio_id: u64) {
    if audio_id == 0 {
        return;
    }

    AUDIO_PLAYERS.with(|players| {
        players.borrow_mut().remove(&audio_id);
    });
}

// ============================================================================
// Renderer FFI Functions
// ============================================================================

/// Create a renderer for a window
/// Returns renderer ID (> 0) on success, 0 on failure
#[no_mangle]
pub extern "C" fn qi_gui_renderer_create_impl(window_id: u64) -> u64 {
    if window_id == 0 {
        return 0;
    }

    let mut state = get_gui_state();
    let Some(state) = state.as_mut() else {
        return 0;
    };

    // Get the window
    let Some(window) = state.created_windows.get(&window_id) else {
        eprintln!("Error: Window ID {} not found", window_id);
        return 0;
    };

    // Create renderer from the window's Arc<Mutex<TaoWindow>>
    let tao_window = window.inner();
    match Renderer::new_from_arc_mutex(tao_window) {
        Ok(renderer) => {
            // Generate renderer ID
            let renderer_id = window_id * 1000 + 1; // Simple ID generation

            // Store renderer in thread-local storage
            RENDERERS.with(|renderers| {
                renderers.borrow_mut().insert(renderer_id, renderer);
            });

            renderer_id
        }
        Err(e) => {
            eprintln!("Failed to create renderer: {}", e);
            0
        }
    }
}

/// Clear the rendering surface with a color (RGB)
#[no_mangle]
pub extern "C" fn qi_gui_renderer_clear_impl(renderer_id: u64, r: u8, g: u8, b: u8) {
    if renderer_id == 0 {
        return;
    }

    RENDERERS.with(|renderers| {
        if let Some(renderer) = renderers.borrow_mut().get_mut(&renderer_id) {
            renderer.clear(r, g, b);
        }
    });
}

/// Draw a filled rectangle
#[no_mangle]
pub extern "C" fn qi_gui_renderer_draw_rect_impl(
    renderer_id: u64,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    r: u8,
    g: u8,
    b: u8,
) {
    if renderer_id == 0 {
        return;
    }

    RENDERERS.with(|renderers| {
        if let Some(renderer) = renderers.borrow_mut().get_mut(&renderer_id) {
            renderer.draw_rect(x, y, width, height, r, g, b);
        }
    });
}

/// Draw a single pixel
#[no_mangle]
pub extern "C" fn qi_gui_renderer_draw_pixel_impl(
    renderer_id: u64,
    x: u32,
    y: u32,
    r: u8,
    g: u8,
    b: u8,
) {
    if renderer_id == 0 {
        return;
    }

    RENDERERS.with(|renderers| {
        if let Some(renderer) = renderers.borrow_mut().get_mut(&renderer_id) {
            renderer.draw_pixel(x, y, r, g, b);
        }
    });
}

/// Draw a line using Bresenham algorithm
#[no_mangle]
pub extern "C" fn qi_gui_renderer_draw_line_impl(
    renderer_id: u64,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    r: u8,
    g: u8,
    b: u8,
) {
    if renderer_id == 0 {
        return;
    }

    RENDERERS.with(|renderers| {
        if let Some(renderer) = renderers.borrow_mut().get_mut(&renderer_id) {
            renderer.draw_line(x0, y0, x1, y1, r, g, b);
        }
    });
}

/// Draw a circle using midpoint circle algorithm
#[no_mangle]
pub extern "C" fn qi_gui_renderer_draw_circle_impl(
    renderer_id: u64,
    cx: i32,
    cy: i32,
    radius: u32,
    r: u8,
    g: u8,
    b: u8,
) {
    if renderer_id == 0 {
        return;
    }

    RENDERERS.with(|renderers| {
        if let Some(renderer) = renderers.borrow_mut().get_mut(&renderer_id) {
            renderer.draw_circle(cx, cy, radius, r, g, b);
        }
    });
}

/// Draw an image from file
/// Returns 0 on success, non-zero on error
#[no_mangle]
pub extern "C" fn qi_gui_renderer_draw_image_impl(
    renderer_id: u64,
    file_path: *const c_char,
    x: u32,
    y: u32,
) -> i32 {
    if renderer_id == 0 || file_path.is_null() {
        return -1;
    }

    let c_str = unsafe { CStr::from_ptr(file_path) };
    let path_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    RENDERERS.with(|renderers| {
        if let Some(renderer) = renderers.borrow_mut().get_mut(&renderer_id) {
            match renderer.draw_image(path_str, x, y) {
                Ok(_) => 0,
                Err(e) => {
                    eprintln!("Failed to draw image: {}", e);
                    -1
                }
            }
        } else {
            -1
        }
    })
}

/// Resize the renderer surface
#[no_mangle]
pub extern "C" fn qi_gui_renderer_resize_impl(renderer_id: u64, width: u32, height: u32) {
    if renderer_id == 0 {
        return;
    }

    RENDERERS.with(|renderers| {
        if let Some(renderer) = renderers.borrow_mut().get_mut(&renderer_id) {
            renderer.resize(width, height);
        }
    });
}

/// Get renderer width
#[no_mangle]
pub extern "C" fn qi_gui_renderer_get_width_impl(renderer_id: u64) -> u32 {
    if renderer_id == 0 {
        return 0;
    }

    RENDERERS.with(|renderers| {
        if let Some(renderer) = renderers.borrow().get(&renderer_id) {
            renderer.size().0
        } else {
            0
        }
    })
}

/// Get renderer height
#[no_mangle]
pub extern "C" fn qi_gui_renderer_get_height_impl(renderer_id: u64) -> u32 {
    if renderer_id == 0 {
        return 0;
    }

    RENDERERS.with(|renderers| {
        if let Some(renderer) = renderers.borrow().get(&renderer_id) {
            renderer.size().1
        } else {
            0
        }
    })
}

/// Free/release a renderer
#[no_mangle]
pub extern "C" fn qi_gui_renderer_free_impl(renderer_id: u64) {
    if renderer_id == 0 {
        return;
    }

    RENDERERS.with(|renderers| {
        renderers.borrow_mut().remove(&renderer_id);
    });
}

/// Draw text at a position with a color
#[no_mangle]
pub extern "C" fn qi_gui_renderer_draw_text_impl(
    renderer_id: u64,
    text: *const c_char,
    x: i32,
    y: i32,
    r: u8,
    g: u8,
    b: u8,
) {
    if renderer_id == 0 || text.is_null() {
        return;
    }

    let c_str = unsafe { CStr::from_ptr(text) };
    let text_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => return,
    };

    RENDERERS.with(|renderers| {
        if let Some(renderer) = renderers.borrow_mut().get_mut(&renderer_id) {
            renderer.draw_text(text_str, x, y, r, g, b);
        }
    });
}

/// Draw text with custom scale
#[no_mangle]
pub extern "C" fn qi_gui_renderer_draw_text_scaled_impl(
    renderer_id: u64,
    text: *const c_char,
    x: i32,
    y: i32,
    scale: u32,
    r: u8,
    g: u8,
    b: u8,
) {
    if renderer_id == 0 || text.is_null() {
        return;
    }

    let c_str = unsafe { CStr::from_ptr(text) };
    let text_str = match c_str.to_str() {
        Ok(s) => s,
        Err(_) => return,
    };

    RENDERERS.with(|renderers| {
        if let Some(renderer) = renderers.borrow_mut().get_mut(&renderer_id) {
            renderer.draw_text_scaled(text_str, x, y, scale, r, g, b);
        }
    });
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        unsafe {
            let version = qi_gui_version_impl();
            assert!(!version.is_null());
            let version_str = CStr::from_ptr(version).to_str().unwrap();
            assert!(version_str.contains("qi-gui"));
            qi_gui_free_string_impl(version);
        }
    }

    #[test]
    fn test_create_window() {
        let title = CString::new("Test Window").unwrap();
        let window_id = qi_gui_create_window_impl(title.as_ptr(), 800, 600);
        assert!(window_id > 0);
    }

    #[test]
    fn test_window_title() {
        let title = CString::new("Test Window").unwrap();
        let window_id = qi_gui_create_window_impl(title.as_ptr(), 800, 600);

        unsafe {
            let retrieved_title = qi_gui_get_title_impl(window_id);
            assert!(!retrieved_title.is_null());
            let title_str = CStr::from_ptr(retrieved_title).to_str().unwrap();
            assert_eq!(title_str, "Test Window");
            qi_gui_free_string_impl(retrieved_title);
        }
    }
}
