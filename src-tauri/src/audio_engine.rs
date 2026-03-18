use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs::File;
use std::path::Path;
use std::sync::{Arc, Mutex};
use stratum_dsp::{analyze_audio, AnalysisConfig};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use thread_priority::*;

struct StreamHandle(#[allow(dead_code)] cpal::Stream);
// SAFETY: cpal::Stream is Send+Sync in practice — it is an opaque handle to an OS audio
// thread. We never access it from multiple threads; it is stored solely to prevent drop.
unsafe impl Send for StreamHandle {}
unsafe impl Sync for StreamHandle {}

pub struct AudioBuffer {
    pub data: Vec<f32>,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration: f32,
    pub bpm: f32,           // Detected BPM
    pub waveform: Vec<f32>, // Downsampled peak magnitudes for UI
}

#[derive(Clone)]
pub struct Voice {
    key: String,
    buffer: Arc<AudioBuffer>,
    position: f64,      // Precise fractional position for resampling
    playback_rate: f64, // Ratio of file SR to device SR
    looping: bool,
    loop_start: f64,
    loop_end: f64,
    gain: f32,
    attack_samples: usize,
    release_samples: usize,
    stopped: bool,
    fade_position: usize, // Current position in the overall envelope
    is_fading_out: bool,
    stop_command: bool,       // Flag to trigger symmetric release
    fade_start_gain: f32,     // Snapshot of gain when fade-out starts
    fade_out_pos: usize,      // Progress of the fade-out specifically
    custom_release_set: bool, // Flag to prevent symmetry override when frontend provides effective_release
}

#[derive(serde::Serialize, Clone, Default)]
pub struct VisualData {
    pub peak: f32,
    pub samples: Vec<f32>,
}

pub struct VisualState {
    pub levels: HashMap<String, VisualData>,
    pub active_keys: Vec<String>,
}

#[derive(serde::Serialize)]
pub struct LevelsResponse {
    pub data: HashMap<String, VisualData>,
    pub active_keys: Vec<String>,
}

pub struct AudioEngineState {
    pub sound_bank: HashMap<String, Arc<AudioBuffer>>,
    pub pad_configs: HashMap<String, PlayParams>, // Persistent pad settings
    voices: Vec<Voice>,
    master_volume: f32,
    pub master_bpm: f32, // Global Master BPM
    sample_rate: u32,    // Device sample rate
}

pub struct AudioEngine {
    state: Arc<Mutex<AudioEngineState>>,
    visuals: Arc<Mutex<VisualState>>,
    _stream: Arc<Mutex<Option<StreamHandle>>>,
}

impl AudioEngine {
    pub fn new() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("No output device found")?;
        let config = device.default_output_config().map_err(|e| e.to_string())?;
        let device_sample_rate = config.sample_rate().0;

        let state = Arc::new(Mutex::new(AudioEngineState {
            sound_bank: HashMap::new(),
            pad_configs: HashMap::new(),
            voices: Vec::new(),
            master_volume: 1.0,
            master_bpm: 120.0,
            sample_rate: device_sample_rate,
        }));

        let visuals = Arc::new(Mutex::new(VisualState {
            levels: HashMap::new(),
            active_keys: Vec::new(),
        }));

        let state_cb = Arc::clone(&state);
        let visuals_cb = Arc::clone(&visuals);
        let channels = config.channels() as usize;

        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config.into(),
                move |data: &mut [f32], _| {
                    // Set thread priority on first run
                    static ONCE: std::sync::Once = std::sync::Once::new();
                    ONCE.call_once(|| {
                        if let Err(e) = set_current_thread_priority(ThreadPriority::Max) {
                            eprintln!("Failed to set audio thread priority: {:?}", e);
                        }
                    });
                    write_audio(data, &state_cb, &visuals_cb, channels)
                },
                |err| eprintln!("Audio stream error: {}", err),
                None,
            ),
            _ => return Err("Unsupported sample format".into()),
        }
        .map_err(|e| e.to_string())?;

        stream.play().map_err(|e| e.to_string())?;

        Ok(Self {
            state,
            visuals,
            _stream: Arc::new(Mutex::new(Some(StreamHandle(stream)))),
        })
    }


    pub async fn load_sound(
        &self,
        key: String,
        path: &str,
        cached_bpm: Option<f32>,
    ) -> Result<LoadResult, String> {
        let path_clone = path.to_string();

        // 1. Decode the file.
        // Note: decode_file still runs its internal 15s analysis,
        // but we will override it immediately if we have a cache.
        let skip = cached_bpm.is_some();
        let mut buffer = tokio::task::spawn_blocking(move || decode_file(&path_clone, skip))
            .await
            .map_err(|e| e.to_string())??;

        // Override with cached BPM if available
        if let Some(bpm) = cached_bpm {
            // println!("[Inner Cosmos] Skipping Analysis for {}. Using Cache: {}", key, bpm);
            buffer.bpm = bpm;
        }

        let result = LoadResult {
            duration: buffer.duration,
            bpm: buffer.bpm,
            waveform: buffer.waveform.clone(),
        };

        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        state.sound_bank.insert(key, Arc::new(buffer));

        Ok(result)
    }

    pub fn get_buffer_waveform(&self, key: &str) -> Vec<f32> {
        if let Ok(state) = self.state.lock() {
            state
                .sound_bank
                .get(key)
                .map(|b| b.waveform.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        }
    }

    pub fn play_sound(&self, key: String, params: PlayParams) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        self.play_sound_locked(&mut state, key, params)
    }

    fn play_sound_locked(
        &self,
        state: &mut AudioEngineState,
        key: String,
        params: PlayParams,
    ) -> Result<(), String> {
        let buffer = state
            .sound_bank
            .get(&key)
            .cloned()
            .ok_or("Sound not found")?;

        let device_sr = state.sample_rate as f64;
        let file_sr = buffer.sample_rate as f64;
        let mut playback_rate = file_sr / device_sr;

        if params.sync && params.sample_bpm > 0.0 {
            let ratio = state.master_bpm / params.sample_bpm;
            playback_rate *= ratio as f64;
        }

        // Convert time params to samples relative to the FILE's sample rate
        let b_channels = buffer.channels as f64;
        let start_pos = params.start_time as f64 * file_sr * b_channels;
        let end_pos = params.end_time as f64 * file_sr * b_channels;

        // Envelope is tracked in DEVICE samples
        let attack_samples = (params.attack as f64 * device_sr) as usize;
        let release_samples = (params.release as f64 * device_sr) as usize;

        state.voices.push(Voice {
            key: key.clone(),
            buffer,
            position: start_pos,
            playback_rate,
            looping: params.looping,
            loop_start: start_pos,
            loop_end: end_pos,
            gain: params.volume,
            attack_samples,
            release_samples,
            stopped: false,
            fade_position: 0,
            is_fading_out: false,
            fade_start_gain: 1.0,
            fade_out_pos: 0,
            stop_command: false,
            custom_release_set: false,
        });

        Ok(())
    }

    pub fn toggle_sound_direct(&self, key: String) -> Result<bool, String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;

        // 1. Check if already playing
        let mut found_active = false;
        for voice in state.voices.iter_mut() {
            if voice.key == key && !voice.stopped && !voice.is_fading_out {
                voice.stop_command = true;
                found_active = true;
            }
        }

        if found_active {
            return Ok(false); // Was playing, now stopping
        }

        // 2. Otherwise, trigger play using stored config
        let params = state
            .pad_configs
            .get(&key)
            .cloned()
            .ok_or_else(|| format!("No configuration found for pad {}", key))?;

        self.play_sound_locked(&mut state, key, params)?;
        Ok(true) // Started playing
    }

    pub fn stop_sound(&self, key: String, effective_release: Option<f32>) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        let device_sr = state.sample_rate as f64;

        for voice in state.voices.iter_mut() {
            if voice.key == key && !voice.stopped && !voice.is_fading_out {
                // If effective_release is provided, override the release duration
                if let Some(eff_rel) = effective_release {
                    voice.release_samples = (eff_rel as f64 * device_sr) as usize;
                    voice.custom_release_set = true; // Prevent symmetry override
                }
                voice.stop_command = true;
            }
        }
        Ok(())
    }

    pub fn update_voice(&self, key: String, params: PlayParams) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;

        // 1. Update the persistent config for future triggers
        state.pad_configs.insert(key.clone(), params.clone());

        // 2. Update all active voices for this key
        for voice in state.voices.iter_mut() {
            if voice.key == key && !voice.stopped {
                let file_sr = voice.buffer.sample_rate as f64;
                let b_channels = voice.buffer.channels as f64;

                voice.gain = params.volume;
                voice.looping = params.looping;
                voice.loop_start = params.start_time as f64 * file_sr * b_channels;
                voice.loop_end = params.end_time as f64 * file_sr * b_channels;
            }
        }
        Ok(())
    }

    pub fn stop_all(&self) {
        if let Ok(mut state) = self.state.lock() {
            for voice in state.voices.iter_mut() {
                if !voice.is_fading_out {
                    voice.is_fading_out = true;
                    voice.fade_out_pos = 0;
                }
            }
        }
    }

    pub fn set_master_volume(&self, volume: f32) {
        if let Ok(mut state) = self.state.lock() {
            state.master_volume = volume;
        }
    }

    pub fn set_master_bpm(&self, bpm: f32) {
        if let Ok(mut state) = self.state.lock() {
            state.master_bpm = bpm;
        }
    }

    pub fn get_levels(&self) -> LevelsResponse {
        if let Ok(visuals) = self.visuals.lock() {
            LevelsResponse {
                data: visuals.levels.clone(),
                active_keys: visuals.active_keys.clone(),
            }
        } else {
            LevelsResponse {
                data: HashMap::new(),
                active_keys: Vec::new(),
            }
        }
    }

    pub fn unload_sound(&self, key: String) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        state.sound_bank.remove(&key);
        state.pad_configs.remove(&key);
        // Also stop any active voices for this key
        for voice in state.voices.iter_mut() {
            if voice.key == key {
                voice.stopped = true;
            }
        }
        Ok(())
    }

    pub fn clear_all_pads(&self) -> Result<(), String> {
        let mut state = self.state.lock().map_err(|e| e.to_string())?;
        state.sound_bank.clear();
        state.pad_configs.clear();
        for voice in state.voices.iter_mut() {
            voice.stopped = true;
        }
        Ok(())
    }
}



#[derive(serde::Serialize)]
pub struct LoadResult {
    pub duration: f32,
    pub bpm: f32,
    pub waveform: Vec<f32>,
}

#[derive(serde::Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlayParams {
    pub volume: f32,
    pub attack: f32,
    pub release: f32,
    pub looping: bool,
    pub start_time: f32,
    pub end_time: f32,
    pub sync: bool,
    pub sample_bpm: f32,
}

fn write_audio(
    data: &mut [f32],
    state_mutex: &Arc<Mutex<AudioEngineState>>,
    visuals_mutex: &Arc<Mutex<VisualState>>,
    channels: usize,
) {
    let mut state = match state_mutex.lock() {
        Ok(s) => s,
        Err(_) => return,
    };

    // --- THE SILENT GUARD ---
    if state.voices.is_empty() {
        data.fill(0.0);
        if let Ok(mut visuals) = visuals_mutex.lock() {
            if !visuals.levels.is_empty() || !visuals.active_keys.is_empty() {
                visuals.levels.clear();
                visuals.active_keys.clear();
            }
        }
        return;
    }

    data.fill(0.0);

    let master = state.master_volume;
    let mut visual_updates = Vec::with_capacity(state.voices.len());

    state.voices.retain_mut(|voice| {
        let mut voice_peak = 0.0f32;
        let mut voice_samples = Vec::with_capacity(128);

        let data_len = voice.buffer.data.len();
        let b_channels = voice.buffer.channels as usize;

        for frame in data.chunks_mut(channels) {
            if voice.stopped {
                break;
            }

            let mut env_gain = 1.0f32;

            // 1. Calculate potential "Attack" gain
            if voice.fade_position < voice.attack_samples {
                env_gain = voice.fade_position as f32 / voice.attack_samples as f32;
            }

            // 2. Handle Stop Command (Manual) with Symmetry
            if voice.stop_command && !voice.is_fading_out {
                if !voice.custom_release_set && voice.fade_position < voice.attack_samples {
                    voice.release_samples = voice.fade_position;
                }
                voice.is_fading_out = true;
                voice.fade_start_gain = env_gain;
                voice.fade_out_pos = 0;
            }

            // 3. Trigger "Natural Release" BEFORE reaching loop_end (One-Shot only)
            if !voice.is_fading_out && !voice.looping {
                let file_samples_remaining = voice.loop_end - voice.position;
                let device_samples_remaining =
                    file_samples_remaining / (voice.playback_rate * b_channels as f64);

                if device_samples_remaining <= voice.release_samples as f64 {
                    voice.is_fading_out = true;
                    voice.fade_start_gain = env_gain;
                    voice.fade_out_pos = 0;
                }
            }

            // 4. Handle Fading
            if voice.is_fading_out {
                if voice.fade_out_pos == 0 {
                    voice.fade_start_gain = env_gain;
                }
                let release_progress = if voice.release_samples > 0 {
                    voice.fade_out_pos as f32 / voice.release_samples as f32
                } else {
                    1.0
                };
                if release_progress >= 1.0 {
                    voice.stopped = true;
                    break;
                }
                env_gain = voice.fade_start_gain * (1.0 - release_progress);
                voice.fade_out_pos += 1;
            } else {
                voice.fade_position += 1;
            }

            let gain = voice.gain * env_gain;
            let mut left_sample = 0.0f32;
            let mut right_sample = 0.0f32;
            let mut s_visual = 0.0f32;

            if b_channels == 1 {
                let pos_idx = voice.position.floor() as usize;
                let frac = (voice.position - pos_idx as f64) as f32;

                if pos_idx >= data_len {
                    voice.stopped = true;
                    break;
                }

                let s1 = voice.buffer.data[pos_idx];
                let s2 = if pos_idx + 1 < data_len {
                    voice.buffer.data[pos_idx + 1]
                } else {
                    0.0
                };
                let s_raw = s1 * (1.0 - frac) + s2 * frac;
                left_sample = s_raw * gain;
                right_sample = left_sample;
                s_visual = s_raw;
                voice.position += voice.playback_rate;
            } else if b_channels >= 2 {
                let base_pos = (voice.position / 2.0).floor() * 2.0;
                let pos_idx = base_pos as usize;
                let frac = ((voice.position - base_pos) / 2.0) as f32;

                if pos_idx + 1 < data_len {
                    let l1 = voice.buffer.data[pos_idx];
                    let l2 = if pos_idx + 2 < data_len {
                        voice.buffer.data[pos_idx + 2]
                    } else {
                        l1
                    };
                    let l_raw = l1 * (1.0 - frac) + l2 * frac;
                    left_sample = l_raw * gain;

                    let r1 = voice.buffer.data[pos_idx + 1];
                    let r2 = if pos_idx + 3 < data_len {
                        voice.buffer.data[pos_idx + 3]
                    } else {
                        r1
                    };
                    let r_raw = r1 * (1.0 - frac) + r2 * frac;
                    right_sample = r_raw * gain;
                    s_visual = (l_raw + r_raw) * 0.5;
                } else {
                    voice.stopped = true;
                    break;
                }
                voice.position += voice.playback_rate * 2.0;
            }

            voice_peak = voice_peak.max(s_visual.abs());
            if voice_samples.len() < 128 {
                voice_samples.push(s_visual);
            }

            if channels == 1 {
                frame[0] += (left_sample + right_sample) * 0.5 * master;
            } else {
                frame[0] += left_sample * master;
                frame[1] += right_sample * master;
            }

            // Handle Looping
            if !voice.is_fading_out
                && voice.looping
                && (voice.position >= voice.loop_end || voice.position >= (data_len as f64))
            {
                voice.position = voice.loop_start;
            }
        }

        visual_updates.push((voice.key.clone(), voice_peak, voice_samples));
        !voice.stopped
    });

    if let Ok(mut visuals) = visuals_mutex.lock() {
        visuals.active_keys.clear();
        for (key, peak, samples) in visual_updates {
            visuals.active_keys.push(key.clone());
            visuals.levels.insert(
                key,
                VisualData {
                    peak,
                    samples,
                },
            );
        }
    }
}


fn decode_file(path: &str, skip_analysis: bool) -> Result<AudioBuffer, String> {
    let src = File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(src), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = Path::new(path).extension() {
        hint.with_extension(&ext.to_string_lossy());
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &symphonia::core::formats::FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| e.to_string())?;

    let mut format_reader = probed.format;
    let (track_id, codec_params) = {
        let track = format_reader
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or("No supported audio track found")?;
        (track.id, track.codec_params.clone())
    };

    let mut decoder = symphonia::default::get_codecs()
        .make(&codec_params, &DecoderOptions::default())
        .map_err(|e| e.to_string())?;

    let mut pcm_data = Vec::new();
    let sample_rate = codec_params.sample_rate.unwrap_or(44100);
    let channels = codec_params.channels.map(|c| c.count() as u16).unwrap_or(2);

    loop {
        let packet = match format_reader.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(e) => return Err(e.to_string()),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = decoder.decode(&packet).map_err(|e| e.to_string())?;
        let mut sample_buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        sample_buf.copy_interleaved_ref(decoded);
        pcm_data.extend_from_slice(sample_buf.samples());
    }

    let duration = pcm_data.len() as f32 / (sample_rate as f32 * channels as f32);

    if channels == 0 {
        return Err("Invalid audio: 0 channels".to_string());
    }

    // ========================================================================
    // BPM Detection Logic Gate
    // ========================================================================
    let mut bpm = 120.0; // Default placeholder if analysis is skipped

    if !skip_analysis {
        // We decimate by a factor of 4. At 48kHz, this gives us 12kHz—perfect for BPM.
        let step = 4;
        let analysis_limit_seconds = 15;
        let max_frames = sample_rate as usize * analysis_limit_seconds;

        let mono_data: Vec<f32> = pcm_data
            .chunks(channels as usize * step)
            .take(max_frames / step)
            .map(|chunk| {
                let mut sum = 0.0;
                for i in 0..channels as usize {
                    sum += chunk[i];
                }
                sum / channels as f32
            })
            .collect();

        let effective_sr = sample_rate / step as u32;
        let mut config = AnalysisConfig::default();
        config.bpm_resolution = 0.1;
        config.enable_bpm_fusion = true;

        let detected_bpm = analyze_audio(&mono_data, effective_sr, config)
            .map(|res| res.bpm)
            .unwrap_or(120.0);

        bpm = if (detected_bpm - detected_bpm.round()).abs() < 0.1 {
            detected_bpm.round()
        } else {
            detected_bpm
        };

    } else {
        // println!("[Inner Cosmos] BPM Analysis skipped for {} (Using Cache)", path);
    }

    // ========================================================================
    // Waveform Generation (Always happens for UI)
    // ========================================================================
    let mut waveform = Vec::with_capacity(400);
    if !pcm_data.is_empty() {
        let step_wf = (pcm_data.len() / (channels as usize)) / 400;
        let step_wf = if step_wf == 0 { 1 } else { step_wf };

        for i in 0..400 {
            let start = i * step_wf * (channels as usize);
            let end = (start + step_wf * (channels as usize)).min(pcm_data.len());
            if start >= pcm_data.len() {
                break;
            }

            let mut peak = 0.0f32;
            for j in start..end {
                peak = peak.max(pcm_data[j].abs());
            }
            waveform.push(peak);
        }
    }

    Ok(AudioBuffer {
        data: pcm_data,
        sample_rate,
        channels,
        duration,
        bpm,
        waveform,
    })
}
