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
use std::thread;
use tauri::{AppHandle, Emitter, Manager, State};
use std::io::Write;

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

/// Start the background keyboard listener in a separate thread
fn start_background_listener(app_handle: tauri::AppHandle) {
    let is_focused_flag = Arc::clone(&app_handle.state::<HotkeyRegistry>().is_focused);
    let enabled = Arc::clone(&app_handle.state::<HotkeyRegistry>().enabled);

    // [Reliability Fix] Use tauri's async runtime for the listener thread
    // as it manages platform-specific capabilities better on macOS.
    tauri::async_runtime::spawn_blocking(move || {
        log_debug("[Listener] Initializing rdev loop...");
        
        // Reverting to listen for all platforms. 
        // We use spawn_blocking to ensure it has proper system context.
        if let Err(error) = rdev::listen(move |event| {
            handle_event(&event, &app_handle, &is_focused_flag, &enabled);
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
    if !enabled.load(Ordering::Relaxed) {
        return;
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
                    audio.stop_all();
                } else {
                    let is_focused = is_focused_flag.load(Ordering::Relaxed);
                    log_debug(&format!("[Rust Hook] Key: {}, Focused: {}", k, is_focused));

                    if !is_focused {
                        if let Ok(res) = audio.toggle_sound_direct(k_string) {
                            is_playing = Some(res);
                        }
                    }
                }
            }

            let _ = app_handle.emit("global-key-press", GlobalKeyPayload { 
                key: k.to_string(), 
                is_playing 
            });
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
