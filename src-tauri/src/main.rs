#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use rdev::{EventType, Key};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::collections::HashSet;
use std::thread;
use tauri::{AppHandle, Emitter, Manager, State};
use std::io::Write;
use std::sync::mpsc::channel;
#[cfg(target_os = "windows")]
use std::sync::mpsc::Sender;

#[cfg(target_os = "windows")]
use windows::Win32::Media::Audio::{
    IAudioSessionControl2, IAudioSessionManager2, ISimpleAudioVolume, MMDeviceEnumerator,
    IMMDeviceEnumerator, IMMDevice, eRender, eMultimedia,
};
#[cfg(target_os = "windows")]
use windows::core::ComInterface;
#[cfg(target_os = "windows")]
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

mod audio_engine;

use crate::audio_engine::{AudioEngine, LevelsResponse, LoadResult};
/**
 * main.rs
 * L-SAMP 100 | Tauri Backend
 * Core Process: Hardware Bridge & File Management
 *
 * This Rust backend provides:
 * - Global hotkey listening
 * - File system operations
 * - IPC command handlers
 * - Native menu system
 */

/// Registry state used for global hotkey management
pub struct HotkeyRegistry {
    /// Fast, lock-free enabled flag checked by callbacks
    pub enabled: Arc<AtomicBool>,
    /// Registered hotkey identifiers (managed under a Mutex)
    pub registrations: Mutex<Vec<String>>,
    /// Previous alert volume on macOS (to restore later)
    pub previous_alert_volume: Mutex<Option<i32>>,
    /// Tracks if the main window is currently focused
    pub is_focused: Arc<AtomicBool>,
    /// Tracks which keys are currently physically pressed to ignore auto-repeat
    pub pressed_keys: Mutex<HashSet<rdev::Key>>,
}

#[derive(Clone, Serialize)]
struct GlobalKeyPayload {
    key: String,
    is_playing: Option<bool>,
}

/// Configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    accent_color: String,
    master_volume: f32,
}

pub const IS_COMMUNITY_BUILD: bool = false; // I am just sitting here

// ============================================================================
// LOGGING UTILITY
// ============================================================================

fn log_debug(msg: &str) {
    if let Some(home) = dirs::home_dir() {
        let log_file = home.join("lsamp-100-debug.log");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
        {
            let _ = writeln!(file, "[{}] {}", chrono::Local::now().format("%H:%M:%S"), msg);
        }
    }
    println!("{}", msg);
}

// ============================================================================
// STATE MANAGEMENT
// ============================================================================

fn main() {
    // Fix for WebKitGTK hardware acceleration issue on Linux (blank window)
    #[cfg(target_os = "linux")]
    std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");

    tauri::Builder::default()
        // Manage a shared hotkey registry: an `AtomicBool` for quick checks
        // and a `Mutex` for safe registration/unregistration operations.
        .manage(HotkeyRegistry {
            enabled: Arc::new(AtomicBool::new(true)),
            registrations: Mutex::new(Vec::new()),
            previous_alert_volume: Mutex::new(None),
            is_focused: Arc::new(AtomicBool::new(true)),
            pressed_keys: Mutex::new(HashSet::new()),
        })
        .manage(AudioEngine::new().expect("Failed to initialize audio engine"))
        .invoke_handler(tauri::generate_handler![
            get_is_community_build,
            get_harbor_files,
            open_audio_folder,
            get_audio_file,
            toggle_listener,
            apply_config,
            select_file,
            toggle_devtools,
            audio_load,
            audio_play,
            audio_stop,
            get_harbor_path,
            audio_get_levels,
            audio_get_waveform,
            audio_set_master_bpm,
            audio_update_params,
            audio_unload,
            audio_clear_all,
            audio_toggle_direct,
            audio_stop_all,
        ])
        .setup(|app| {
            let app_handle = app.handle().clone();
            start_background_listener(app_handle.clone());
            
            // Ensure muzzling is active on startup since enabled defaults to true
            muzzle_system_sounds(true, &app_handle);

            #[cfg(target_os = "macos")]
            {
                macos_native::disable_app_nap();
                
                if let Some(window) = app.get_webview_window("main") {
                    // macOS titlebar is ~28px. We add it to the base 736px height
                    // to ensure the internal webview area remains exactly 736px.
                    let _ = window.set_size(tauri::Size::Logical(tauri::LogicalSize {
                        width: 1252.0,
                        height: 764.0,
                    }));
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(focused) = event {
                let registry = window.state::<HotkeyRegistry>();
                registry.is_focused.store(*focused, Ordering::SeqCst);
                log_debug(&format!("[Focus] Main window focused: {}", focused));
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ============================================================================
// SYSTEM SOUND MUZZLING (The Clever Trick)
// ============================================================================

/// Muzzle system notification sounds briefly to prevent the "beep" on Windows/macOS
/// without capturing the keyboard events globally.
/// Muzzle system notification sounds persistently while sensing is active.
fn muzzle_system_sounds(mute: bool, app_handle: &tauri::AppHandle) {
    log_debug(&format!("[Muzzle] Entering muzzle (sync part) with mute={}", mute));
    
    #[cfg(target_os = "macos")]
    {
        log_debug("[Muzzle] Target detected: macOS");
        let registry = app_handle.state::<HotkeyRegistry>();
        if mute {
            // Store current volume before muting
            let output = std::process::Command::new("osascript")
                .arg("-e")
                .arg("get volume alert volume")
                .output();
            
            if let Ok(out) = output {
                let vol_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if let Ok(vol) = vol_str.parse::<i32>() {
                    let mut prev = registry.previous_alert_volume.lock().unwrap();
                    *prev = Some(vol);
                    log_debug(&format!("[Muzzle] Stored previous alert volume: {}", vol));
                }
            }

            let _ = std::process::Command::new("osascript")
                .arg("-e")
                .arg("set volume alert volume 0")
                .spawn();
            log_debug("[Muzzle] macOS Alert Volume set to 0");
        } else {
            // Restore previous volume
            let mut prev = registry.previous_alert_volume.lock().unwrap();
            let vol_to_restore = prev.unwrap_or(75);
            let _ = std::process::Command::new("osascript")
                .arg("-e")
                .arg(format!("set volume alert volume {}", vol_to_restore))
                .spawn();
            *prev = None;
            log_debug(&format!("[Muzzle] macOS Alert Volume restored to {}", vol_to_restore));
        }
    }

    #[cfg(target_os = "windows")]
    {
        log_debug("[Muzzle] Target detected: Windows");
        // On Windows, we find the "System Sounds" audio session and mute it.
        thread::spawn(move || unsafe {
            log_debug("[Muzzle-TX] Windows muzzle thread spawned");
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator = match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                Ok(e) => e,
                Err(e) => { log_debug(&format!("[Muzzle] ERR: CoCreateInstance failed: {:?}", e)); return; },
            };
            let device: IMMDevice = match enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) {
                Ok(d) => d,
                Err(e) => { log_debug(&format!("[Muzzle] ERR: GetDefaultAudioEndpoint failed: {:?}", e)); return; },
            };
            let manager: IAudioSessionManager2 = match device.Activate(CLSCTX_ALL, None) {
                Ok(m) => m,
                Err(e) => { log_debug(&format!("[Muzzle] ERR: Activate failed: {:?}", e)); return; },
            };
            if let Ok(session_enumerator) = manager.GetSessionEnumerator() {
                let count = session_enumerator.GetCount().unwrap_or(0);
                log_debug(&format!("[Muzzle] Found {} audio sessions", count));
                for i in 0..count {
                    if let Ok(session) = session_enumerator.GetSession(i) {
                        if let Ok(session2) = session.cast::<IAudioSessionControl2>() {
                            if let Ok(name) = session2.GetSessionIdentifier() {
                                let name_str = name.to_string().unwrap_or_default();
                                log_debug(&format!("[Muzzle-TX] Checking session: '{}'", name_str));
                                
                                // THE SLEDGEHAMMER: Mute every session that is NOT our own.
                                // This is the only way to reliably catch all system dings across different Windows versions.
                                if !name_str.contains("lsamp-100.exe") {
                                    if let Ok(volume) = session.cast::<ISimpleAudioVolume>() {
                                        let _ = volume.SetMute(mute, std::ptr::null());
                                        log_debug(&format!("[Muzzle-TX] SUCCESS: Muted external session: '{}'", name_str));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });
    }

    #[cfg(target_os = "linux")]
    {
        log_debug("[Muzzle] Target detected: Linux (No-op)");
    }
}

// ============================================================================
// GLOBAL BACKGROUND LISTENER (using rdev)
// ============================================================================

// ----------------------------------------------------------------------------
// macOS Native Listener (Bypasses rdev's non-thread-safe name resolution)
// ----------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos_native {
    use std::ffi::c_void;
    use std::sync::mpsc::Sender;
    use rdev::{Event, EventType, Key};
    use crate::log_debug;
    use std::sync::atomic::{AtomicPtr, Ordering};
    use std::ptr;

    type CGEventTapProxy = *mut c_void;
    type CGEventRef = *mut c_void;
    type CFMachPortRef = *mut c_void;
    type CFRunLoopSourceRef = *mut c_void;
    type CFRunLoopRef = *mut c_void;
    type CFStringRef = *mut c_void;
    type ObjcId = *mut c_void;

    // Constants for event tap management
    const kCGEventTapDisabledByTimeout: u32 = 0x1FFFFFFF;
    const kCGEventTapDisabledByUserInput: u32 = 0x1FFFFFFE;

    type CGEventTapCallBack = extern "C" fn(
        proxy: CGEventTapProxy,
        type_: u32,
        event: CGEventRef,
        user_info: *mut c_void,
    ) -> CGEventRef;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            events_interesting: u64,
            callback: CGEventTapCallBack,
            user_info: *mut c_void,
        ) -> CFMachPortRef;
        fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;
        fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFMachPortCreateRunLoopSource(allocator: *mut c_void, tap: CFMachPortRef, order: isize) -> CFRunLoopSourceRef;
        fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        fn CFRunLoopAddSource(rl: CFRunLoopRef, source: CFRunLoopSourceRef, mode: CFStringRef);
        fn CFRunLoopRun();
        static kCFRunLoopCommonModes: CFStringRef;
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    #[link(name = "AppKit", kind = "framework")]
    extern "C" {
        static NSDefaultRunLoopMode: ObjcId;
    }

    #[link(name = "Foundation", kind = "framework")]
    extern "C" {
        fn objc_getClass(name: *const u8) -> ObjcId;
        fn sel_registerName(name: *const u8) -> ObjcId;
        fn objc_msgSend(obj: ObjcId, sel: ObjcId, ...) -> ObjcId;
    }

    /// Context passed to the raw callback
    struct TapContext {
        tx: Sender<Event>,
        tap: AtomicPtr<c_void>,
    }

    /// Disable App Nap for the current process to ensure background threads keep running.
    pub fn disable_app_nap() {
        unsafe {
            let process_info_class = objc_getClass("NSProcessInfo\0".as_ptr());
            let process_info_sel = sel_registerName("processInfo\0".as_ptr());
            let process_info = objc_msgSend(process_info_class, process_info_sel);

            let begin_activity_sel = sel_registerName("beginActivityWithOptions:reason:\0".as_ptr());
            let reason_class = objc_getClass("NSString\0".as_ptr());
            let string_with_utf8_sel = sel_registerName("stringWithUTF8String:\0".as_ptr());
            let reason = objc_msgSend(reason_class, string_with_utf8_sel, "Hotkey Listener Persistence\0".as_ptr());

            // NSActivityBackground (0xFF) | NSActivityLatencyCritical (0xFF00000000)
            let options: u64 = 0x0000_00FF | 0x0000_00FF_0000_0000;
            
            let _ = objc_msgSend(process_info, begin_activity_sel, options, reason);
            log_debug("[Listener] macOS App Nap DISABLED (Solid Mode)");
        }
    }

    fn map_keycode(code: u64) -> Option<Key> {
        match code {
            0 => Some(Key::KeyA),
            1 => Some(Key::KeyS),
            2 => Some(Key::KeyD),
            3 => Some(Key::KeyF),
            6 => Some(Key::KeyZ),
            7 => Some(Key::KeyX),
            8 => Some(Key::KeyC),
            9 => Some(Key::KeyV),
            12 => Some(Key::KeyQ),
            13 => Some(Key::KeyW),
            14 => Some(Key::KeyE),
            15 => Some(Key::KeyR),
            49 => Some(Key::Space),
            _ => None,
        }
    }

    pub fn listen(tx: Sender<Event>) {
        if unsafe { !AXIsProcessTrusted() } {
            log_debug("[Listener] WARNING: Accessibility permissions NOT granted. Event Tap will likely fail.");
        }

        let mask = (1u64 << 10) | (1u64 << 11); // KeyDown (10) and KeyUp (11)
        
        unsafe {
            let context = Box::new(TapContext {
                tx: tx.clone(),
                tap: AtomicPtr::new(ptr::null_mut()),
            });
            let context_ptr = Box::into_raw(context);
            
            // 1. Setup Event Tap (HID Level 0)
            let mut tap = CGEventTapCreate(0, 0, 0, mask, raw_callback, context_ptr as *mut c_void);
            if tap.is_null() {
                log_debug("[Listener] HID tap failed. Trying Session tap...");
                tap = CGEventTapCreate(1, 0, 0, mask, raw_callback, context_ptr as *mut c_void);
            }

            if !tap.is_null() {
                (*context_ptr).tap.store(tap, Ordering::SeqCst);
                let source = CFMachPortCreateRunLoopSource(ptr::null_mut(), tap, 0);
                let run_loop = CFRunLoopGetCurrent();
                // Add to Common Modes to ensure it runs during menu open/modal states
                CFRunLoopAddSource(run_loop, source, kCFRunLoopCommonModes);
                CGEventTapEnable(tap, true);
                log_debug("[Listener] macOS Event Tap ACTIVE");

                // 2. Watchdog: re-enable tap every 500ms in case the OS silently disables it
                start_tap_watchdog(tap);
            } else {
                log_debug("[Listener] ERROR: Both HID and Session taps failed. No key events will be received.");
            }

            log_debug("[Listener] macOS Native Loop is RUNNING (Solid Mode)");
            CFRunLoopRun();
        }
    }

    /// Spawns a watchdog thread that re-enables the EventTap every 500ms.
    /// CGEventTapEnable is thread-safe per Apple docs so this is safe to call from a background thread.
    fn start_tap_watchdog(tap: CFMachPortRef) {
        // Cast to usize so the value is Send (raw pointers are not Send by default).
        // SAFETY: The tap lives for the entire duration of the CFRunLoop on its
        // dedicated thread, so the pointer remains valid for the watchdog's lifetime.
        let tap_addr = tap as usize;
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                if tap_addr != 0 {
                    unsafe { CGEventTapEnable(tap_addr as CFMachPortRef, true) };
                }
            }
        });
        log_debug("[Listener] EventTap watchdog ACTIVE (500ms heartbeat)");
    }

    extern "C" fn raw_callback(
        _proxy: CGEventTapProxy,
        type_: u32,
        event: CGEventRef,
        user_info: *mut c_void,
    ) -> CGEventRef {
        let context = unsafe { &*(user_info as *const TapContext) };

        if type_ == kCGEventTapDisabledByTimeout || type_ == kCGEventTapDisabledByUserInput {
            let tap = context.tap.load(Ordering::SeqCst);
            if !tap.is_null() {
                log_debug("[Listener] Event tap disabled by OS. Re-enabling...");
                unsafe { CGEventTapEnable(tap, true) };
            }
            return event;
        }

        if type_ != 10 && type_ != 11 {
            return event;
        }

        let code = unsafe { CGEventGetIntegerValueField(event, 62) } as u64;
        if let Some(key) = map_keycode(code) {
            let ev_type = match type_ {
                10 => EventType::KeyPress(key),
                11 => EventType::KeyRelease(key),
                _ => return event,
            };
            let e = Event {
                event_type: ev_type,
                time: std::time::SystemTime::now(),
                name: None,
            };
            let _ = context.tx.send(e);
        }
        event
    }
}

fn start_background_listener(app_handle: tauri::AppHandle) {
    let is_focused_flag = Arc::clone(&app_handle.state::<HotkeyRegistry>().is_focused);
    let enabled = Arc::clone(&app_handle.state::<HotkeyRegistry>().enabled);
    
    // Create a channel to move event processing OFF the high-priority keyboard thread.
    // This is CRITICAL for macOS stability (avoids EventTap timeouts/crashes).
    let (tx, rx) = channel::<rdev::Event>();

    // Thread 1: The Processor (handles audio and logging)
    let p_handle = app_handle.clone();
    let p_focus = is_focused_flag.clone();
    let p_enabled = enabled.clone();
    thread::spawn(move || {
        // Set Max priority for the processor to ensure audio triggers instantly
        if let Err(e) = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Max) {
            log_debug(&format!("[Listener] WARNING: Failed to set processor priority: {:?}", e));
        }
        log_debug("[Listener] Processor thread started (Max Priority)");
        while let Ok(event) = rx.recv() {
            handle_event(&event, &p_handle, &p_focus, &p_enabled);
        }
    });

    // Thread 2: The Listener (purely for tapping keys)
    #[cfg(target_os = "macos")]
    thread::spawn(move || {
        // High priority for the listener to avoid being napped/throttled
        if let Err(e) = thread_priority::set_current_thread_priority(thread_priority::ThreadPriority::Max) {
            log_debug(&format!("[Listener] WARNING: Failed to set listener priority: {:?}", e));
        }
        log_debug("[Listener] Initializing NATIVE macOS loop (Solid mode)");
        macos_native::listen(tx);
    });

    #[cfg(not(target_os = "macos"))]
    thread::spawn(move || {
        log_debug("[Listener] Initializing rdev loop...");
        
        let tx_tap = tx.clone();
        if let Err(error) = rdev::listen(move |event| {
            // Send the event immediately and return. DO NOT do any work here.
            let _ = tx_tap.send(event);
        }) {
            log_debug(&format!("[Listener] FATAL Error: {:?}", error));
        }
    });
}

/// Common event handler for both grab and listen
fn handle_event(
    event: &rdev::Event, 
    app_handle: &tauri::AppHandle, 
    is_focused_flag: &Arc<AtomicBool>, 
    enabled: &Arc<AtomicBool>
) {
    let registry = app_handle.state::<HotkeyRegistry>();
    if !enabled.load(Ordering::Relaxed) {
        return;
    }

    // --- AUTO-REPEAT GUARD ---
    match event.event_type {
        EventType::KeyPress(key) => {
            let mut pressed = registry.pressed_keys.lock().unwrap();
            if pressed.contains(&key) {
                // log_debug(&format!("[Hook] Ignoring auto-repeat for {:?}", key));
                return; // Ignore auto-repeat KeyPress
            }
            pressed.insert(key);
            log_debug(&format!("[Hook] KeyPress: {:?}", key));
        }
        EventType::KeyRelease(key) => {
            let mut pressed = registry.pressed_keys.lock().unwrap();
            pressed.remove(&key);
            log_debug(&format!("[Hook] KeyRelease: {:?}", key));
            return; // We only care about KeyPress for actual triggering
        }
        _ => return,
    }

    if let EventType::KeyPress(key) = event.event_type {
        let key_str = match key {
            Key::KeyQ => Some("Q"),
            Key::KeyW => Some("W"),
            Key::KeyE => Some("E"),
            Key::KeyR => Some("R"),
            Key::KeyA => Some("A"),
            Key::KeyS => Some("S"),
            Key::KeyD => Some("D"),
            Key::KeyF => Some("F"),
            Key::KeyZ => Some("Z"),
            Key::KeyX => Some("X"),
            Key::KeyC => Some("C"),
            Key::KeyV => Some("V"),
            Key::Space => Some("SPACE"),
            // Modifier keys - specifically skip to avoid rdev crashes on macOS
            Key::ControlLeft | Key::ControlRight | Key::ShiftLeft | Key::ShiftRight | 
            Key::Alt | Key::AltGr | Key::MetaLeft | Key::MetaRight | Key::Function => {
                return;
            },
            _ => None,
        };

        if let Some(k) = key_str {
            let k_string = k.to_string();
            let audio = app_handle.state::<audio_engine::AudioEngine>();

            let mut permitted = true;
            if IS_COMMUNITY_BUILD && !["Q", "W", "E", "R"].contains(&k) {
                permitted = false;
            }

            let mut is_playing = None;

            if permitted {
                if k == "SPACE" {
                    log_debug("[Hook] Stop All (Space)");
                    audio.stop_all();
                } else {
                    let is_focused = is_focused_flag.load(Ordering::Relaxed);
                    log_debug(&format!("[Hook] Key: {}, Focused: {}", k, is_focused));
                    
                    if !is_focused {
                        if let Ok(res) = audio.toggle_sound_direct(k_string) {
                            is_playing = Some(res);
                            log_debug(&format!("[Hook] Toggled sound: {} (New State: {})", k, res));
                        } else {
                            log_debug(&format!("[Hook] Failed to toggle sound: {}", k));
                        }
                    }
                }

                // Push to frontend for UI updates
                let _ = app_handle.emit("global-key-press", GlobalKeyPayload { 
                    key: k.to_string(), 
                    is_playing 
                });
            }
        }
    }
}

// ============================================================================
// FILE OPERATIONS (Harbor Management)
// ============================================================================

/// Get the audio harbor directory path
fn get_audio_harbor(_app_handle: &AppHandle) -> Result<PathBuf, String> {
    // Use standard config directory: ~/.config/lsamp-100/audio (on Linux)
    let config_dir = dirs::config_dir()
        .ok_or("Failed to get config dir".to_string())?
        .join("lsamp-100");

    let harbor_path = config_dir.join("audio");

    // Ensure the directory exists
    if !harbor_path.exists() {
        fs::create_dir_all(&harbor_path)
            .map_err(|e| format!("[Inner Cosmos] Harbor creation failed: {}", e))?;
        // println!("[Inner Cosmos] Harbor created at: {:?}", harbor_path);
    }

    Ok(harbor_path)
}

/// Recursively scan directory for audio files
fn scan_harbor(dir_path: &PathBuf) -> Result<Vec<String>, String> {
    let mut audio_files = Vec::new();

    fn scan_recursive(
        dir: &PathBuf,
        base_dir: &PathBuf,
        files: &mut Vec<String>,
    ) -> Result<(), String> {
        let entries =
            fs::read_dir(dir).map_err(|e| format!("[Social Noise] Harbor scan failed: {}", e))?;

        for entry in entries {
            let entry = entry.map_err(|e| format!("[Social Noise] Entry error: {}", e))?;
            let path = entry.path();

            if path.is_dir() {
                scan_recursive(&path, base_dir, files)?;
            } else {
                if let Some(ext) = path.extension() {
                    let ext_str = ext.to_string_lossy().to_lowercase();
                    if matches!(ext_str.as_str(), "mp3" | "wav" | "ogg" | "flac") {
                        if let Ok(rel_path) = path.strip_prefix(base_dir) {
                            files.push(rel_path.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    scan_recursive(dir_path, dir_path, &mut audio_files)?;
    Ok(audio_files)
}

/// IPC Command: Get all audio files from harbor
#[tauri::command]
async fn get_harbor_files(app_handle: AppHandle) -> Result<Vec<String>, String> {
    let harbor_path = get_audio_harbor(&app_handle)?;
    scan_harbor(&harbor_path)
}

#[tauri::command]
fn get_is_community_build() -> bool {
    IS_COMMUNITY_BUILD
}

#[tauri::command]
async fn get_harbor_path(app_handle: AppHandle) -> Result<String, String> {
    let path = get_audio_harbor(&app_handle)?;
    Ok(path.to_string_lossy().to_string())
}

/// IPC Command: Open the audio folder in file explorer
#[tauri::command]
async fn open_audio_folder(app_handle: AppHandle) -> Result<(), String> {
    let harbor_path = get_audio_harbor(&app_handle)?;

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(harbor_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(harbor_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(harbor_path)
            .spawn()
            .map_err(|e| format!("Failed to open folder: {}", e))?;
    }

    Ok(())
}

// ============================================================================
// KEYBOARD CONTROL
// ============================================================================

/// IPC Command: Toggle global keyboard listener state
#[tauri::command]
fn toggle_listener(
    state: bool,
    registry: State<'_, HotkeyRegistry>,
    app_handle: AppHandle,
) -> Result<(), String> {
    // Fast, lock-free publish of the enabled/disabled state so any callbacks
    // that are racing with unregister can short-circuit quickly.
    registry.enabled.store(state, Ordering::SeqCst);

    // [Persistent Muzzle] Apply or remove muzzle based on state
    muzzle_system_sounds(state, &app_handle);

    // Now manage registration lifecycle under a mutex to avoid races
    // when adding/removing OS-level hooks. The actual register/unregister
    // calls are plugin-specific and should be placed where indicated below.
    let mut regs = registry.registrations.lock().map_err(|e| e.to_string())?;

    if state {
        // If enabling, (re-)register all required hotkeys. This is a good
        // place to call into a global-shortcut plugin to register keys and
        // store their tokens/ids in `regs` for later unregistration.

        // Example (pseudocode - plugin-specific):
        // let token = global_shortcut::register(&app_handle, "Q", || { /* emit event */ });
        // regs.push(token);

        // For now we store the logical key names so the registration intent is tracked.
        if regs.is_empty() {
            regs.push("Q".to_string());
            regs.push("W".to_string());
            regs.push("E".to_string());
            regs.push("R".to_string());
            regs.push("A".to_string());
            regs.push("S".to_string());
            regs.push("D".to_string());
            regs.push("F".to_string());
            regs.push("Z".to_string());
            regs.push("X".to_string());
            regs.push("C".to_string());
            regs.push("V".to_string());
            regs.push("SPACE".to_string());
        }
    } else {
        // If disabling, unregister all OS hooks. Use plugin API to unregister
        // any tokens stored in `regs`. After successful unregistration clear the list.

        // Example (pseudocode - plugin-specific):
        // for token in regs.iter() { global_shortcut::unregister(token); }

        regs.clear();
    }

    // println!("[Consonance] Keyboard sensing: {}", if state { "ACTIVE" } else { "RELEASED" });
    Ok(())
}

// ============================================================================
// AUDIO FILE SERVING
// ============================================================================

#[tauri::command]
async fn get_audio_file(file_name: String, app_handle: AppHandle) -> Result<Vec<u8>, String> {
    let harbor_path = get_audio_harbor(&app_handle)?;
    let p = PathBuf::from(&file_name);

    let file_path = if p.is_absolute() {
        p
    } else {
        let path = harbor_path.join(&file_name);
        // Security: Prevent path traversal for relative paths
        if !path.starts_with(&harbor_path) {
            return Err("Path traversal detected".to_string());
        }
        path
    };

    if !file_path.exists() {
        return Err(format!("File not found: {:?}", file_path));
    }

    fs::read(&file_path).map_err(|e| format!("[Social Noise] File read failed: {}", e))
}

// ============================================================================
// CONFIGURATION
// ============================================================================

// ============================================================================
// FILE PICKER
// ============================================================================

/// IPC Command: Open native file dialog to pick an audio file
#[tauri::command]
async fn select_file() -> Result<String, String> {
    let file = rfd::AsyncFileDialog::new()
        .add_filter("Audio", &["mp3", "wav", "ogg", "flac"])
        .pick_file()
        .await;

    match file {
        Some(handle) => Ok(handle.path().to_string_lossy().to_string()),
        None => Err("User cancelled".to_string()),
    }
}

/// IPC Command: Toggle Developer Tools
#[tauri::command]
fn toggle_devtools(app_handle: AppHandle) -> Result<(), String> {
    if let Some(window) = app_handle.get_webview_window("main") {
        if window.is_devtools_open() {
            window.close_devtools();
        } else {
            window.open_devtools();
        }
        Ok(())
    } else {
        Err("Main window not found".to_string())
    }
}

/// IPC Command: Apply configuration changes
#[tauri::command]
fn apply_config(
    config: AppConfig,
    audio: State<'_, AudioEngine>,
    _app_handle: AppHandle,
) -> Result<(), String> {
    // In Tauri 2, event emission to windows is handled differently
    // The config is accepted and logged; frontend state management handles it
    // println!("[Config] Applied: {:?}", config);
    audio.inner().set_master_volume(config.master_volume);
    Ok(())
}

// ============================================================================
// AUDIO CONTROL COMMANDS
// ============================================================================

#[tauri::command]
async fn audio_load(
    key: String,
    path: String,
    // This tells Serde to look for 'cachedBpm' from the frontend
    cached_bpm: Option<f32>, // Add this parameter to add bpm caching
    audio: State<'_, AudioEngine>,
) -> Result<LoadResult, String> {
    if IS_COMMUNITY_BUILD && !["Q", "W", "E", "R"].contains(&key.as_str()) {
        // println!("[Bridge] BLOCKED Community Build Request: {}", key);
        return Err("This pad is restricted in the Community Build.".to_string());
    }
    // println!("[Bridge] Request: {} | Cached BPM: {:?}", key, cached_bpm);
    audio.inner().load_sound(key, &path, cached_bpm).await
}

#[tauri::command]
async fn audio_toggle_direct(key: String, audio: State<'_, AudioEngine>) -> Result<bool, String> {
    audio.inner().toggle_sound_direct(key)
}

#[tauri::command]
fn audio_stop_all(audio: State<'_, AudioEngine>) -> Result<(), String> {
    audio.inner().stop_all();
    Ok(())
}

#[tauri::command]
async fn audio_unload(key: String, audio: State<'_, AudioEngine>) -> Result<(), String> {
    audio.inner().unload_sound(key)
}

#[tauri::command]
async fn audio_clear_all(audio: State<'_, AudioEngine>) -> Result<(), String> {
    audio.inner().clear_all_pads()
}

#[tauri::command]
async fn audio_play(
    key: String,
    params: crate::audio_engine::PlayParams,
    audio: State<'_, AudioEngine>,
) -> Result<(), String> {
    if IS_COMMUNITY_BUILD && !["Q", "W", "E", "R"].contains(&key.as_str()) {
        // println!("[AudioPlay] BLOCKED Community Build Play: {}", key);
        return Err("This pad is restricted in the Community Build.".to_string());
    }
    // println!("[AudioPlay] Key: {}, Params: {:?}", key, params);
    audio.inner().play_sound(key, params)
}

#[tauri::command]
async fn audio_stop(
    key: String,
    effective_release: Option<f32>,
    audio: State<'_, AudioEngine>,
) -> Result<(), String> {
    audio.inner().stop_sound(key, effective_release)
}

#[tauri::command]
async fn audio_update_params(
    key: String,
    params: crate::audio_engine::PlayParams,
    audio: State<'_, AudioEngine>,
) -> Result<(), String> {
    // println!("[AudioUpdate] Key: {}, Params: {:?}", key, params);
    audio.inner().update_voice(key, params)
}

#[tauri::command]
async fn audio_get_levels(audio: State<'_, AudioEngine>) -> Result<LevelsResponse, String> {
    Ok(audio.inner().get_levels())
}

#[tauri::command]
async fn audio_set_master_bpm(bpm: f32, audio: State<'_, AudioEngine>) -> Result<(), String> {
    audio.inner().set_master_bpm(bpm);
    Ok(())
}

#[tauri::command]
async fn audio_get_waveform(
    key: String,
    audio: State<'_, AudioEngine>,
) -> Result<Vec<f32>, String> {
    Ok(audio.inner().get_buffer_waveform(&key))
}
