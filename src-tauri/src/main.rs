#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

use rdev::{listen as rdev_listen, EventType, Key};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use tauri::{AppHandle, Emitter, Manager, State};

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
}

/// Configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    accent_color: String,
    master_volume: f32,
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
        })
        .manage(AudioEngine::new().expect("Failed to initialize audio engine"))
        .invoke_handler(tauri::generate_handler![
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
        ])
        .setup(|app| {
            let app_handle = app.handle().clone();
            start_background_listener(app_handle);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ============================================================================
// GLOBAL BACKGROUND LISTENER (using rdev)
// ============================================================================

/// Start the background keyboard listener in a separate thread
fn start_background_listener(app_handle: tauri::AppHandle) {
    let enabled = Arc::clone(&app_handle.state::<HotkeyRegistry>().enabled);

    thread::spawn(move || {
        rdev_listen(move |event| {
            if !enabled.load(Ordering::Relaxed) {
                return;
            }

            if let EventType::KeyPress(key) = event.event_type {
                // Map rdev Key to a String for Angular
                let key_str = match key {
                    // Row 1
                    Key::KeyQ => Some("Q"),
                    Key::KeyW => Some("W"),
                    Key::KeyE => Some("E"),
                    Key::KeyR => Some("R"),

                    // Row 2
                    Key::KeyA => Some("A"),
                    Key::KeyS => Some("S"),
                    Key::KeyD => Some("D"),
                    Key::KeyF => Some("F"),

                    // Row 3
                    Key::KeyZ => Some("Z"),
                    Key::KeyX => Some("X"),
                    Key::KeyC => Some("C"),
                    Key::KeyV => Some("V"),

                    // Global Stop
                    Key::Space => Some("SPACE"),

                    _ => None,
                };

                if let Some(k) = key_str {
                    if k == "SPACE" {
                        let audio = app_handle.state::<AudioEngine>();
                        audio.stop_all();
                    }
                    let _ = app_handle.emit("global-key-press", k);
                }
            }
        })
        .expect("[Consonance] Could not spy on keyboard");
    });
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
        println!("[Inner Cosmos] Harbor created at: {:?}", harbor_path);
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
    _app_handle: AppHandle,
) -> Result<(), String> {
    // Fast, lock-free publish of the enabled/disabled state so any callbacks
    // that are racing with unregister can short-circuit quickly.
    registry.enabled.store(state, Ordering::SeqCst);

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

    println!(
        "[Consonance] Keyboard sensing: {}",
        if state { "ACTIVE" } else { "RELEASED" }
    );
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
    println!("[Config] Applied: {:?}", config);
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
    // DIAGNOSTIC: This MUST show Some(val) for the optimization to work
    println!("[Bridge] Request: {} | Cached BPM: {:?}", key, cached_bpm);
    // audio.inner().load_sound(key, &path).await
    audio.inner().load_sound(key, &path, cached_bpm).await // Replaced the above line with this
}

#[tauri::command]
async fn audio_play(
    key: String,
    params: crate::audio_engine::PlayParams,
    audio: State<'_, AudioEngine>,
) -> Result<(), String> {
    println!("[AudioPlay] Key: {}, Params: {:?}", key, params);
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
    println!("[AudioUpdate] Key: {}, Params: {:?}", key, params);
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
