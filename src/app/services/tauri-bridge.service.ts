/**
 * tauri-bridge.service.ts
 * L-SAMP 100 | Tauri Bridge Service
 * Purpose: Unified API for frontend ↔ Rust backend communication
 */

import { Injectable, NgZone, OnDestroy } from '@angular/core';
import { Subject } from 'rxjs';

// Tauri types - we'll declare them to avoid import issues
type EventPayload<T> = { payload: T };
type UnlistenFn = () => void;
type InvokeFn = <T = any>(cmd: string, args?: any) => Promise<T>;
type ListenFn = <T = any>(event: string, handler: (event: EventPayload<T>) => void) => Promise<UnlistenFn>;

export interface AppConfig {
  accentColor: string;
  masterVolume: number;
}

export interface VisualData {
  peak: number;
  samples: number[];
}

export interface LevelsResponse {
  data: Record<string, VisualData>;
  active_keys: string[];
}

@Injectable({
  providedIn: 'root',
})
export class TauriBridgeService implements OnDestroy {
  // Event subjects for frontend subscriptions
  onKeyTriggered = new Subject<string>();
  onGlobalStop = new Subject<void>();
  onApplyConfig = new Subject<AppConfig>();
  onOpenModal = new Subject<any>();

  private listeners: Array<() => void> = [];
  private tauriReady = false;
  private invoke: any;
  private listen: any;
  private listenerActive = true;

  constructor(private ngZone: NgZone) {
    this.initializeEventListeners();
  }

  /**
   * Wait for Tauri to be ready before invoking commands
   */
  private async waitForReady(): Promise<void> {
    for (let i = 0; i < 50; i++) {
      if (this.tauriReady && this.invoke) {
        return;
      }
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    throw new Error('Tauri initialization timeout');
  }

  /**
   * Initialize all Tauri event listeners
   */
  private async initializeEventListeners(): Promise<void> {
    try {
      // Dynamically import Tauri API
      const coreModule = await import('@tauri-apps/api/core');
      const eventModule = await import('@tauri-apps/api/event');

      this.invoke = coreModule.invoke;
      this.listen = eventModule.listen;
      this.tauriReady = true;

      // Setup native keyboard listeners for QWER ASDF ZXCV
      this.setupKeyboardListeners();

      // Listen for keyboard triggers from Rust backend (Pads)
      const keyTriggerUnlisten = await this.listen('key-triggered', (event: any) => {
        this.ngZone.run(() => {
          this.onKeyTriggered.next(event.payload);
        });
      });

      // Listen for the new global-key-press from rdev (Background)
      const globalKeyPressUnlisten = await this.listen('global-key-press', (event: any) => {
        const key = event.payload;
        this.ngZone.run(() => {
          if (key === 'SPACE') {
            this.onGlobalStop.next();
          } else {
            this.onKeyTriggered.next(key);
          }
        });
      });

      // Listen for global stop (legacy/redundant if rdev covers it)
      const globalStopUnlisten = await this.listen('global-stop', () => {
        this.ngZone.run(() => {
          this.onGlobalStop.next();
        });
      });

      // Listen for config updates from Rust backend
      const configUnlisten = await this.listen('apply-config', (event: any) => {
        this.ngZone.run(() => {
          this.onApplyConfig.next(event.payload);
        });
      });

      // Store unlisten functions for cleanup
      this.listeners = [
        keyTriggerUnlisten,
        globalKeyPressUnlisten,
        globalStopUnlisten,
        configUnlisten
      ];
    } catch (error) {
      console.error('[TauriBridge] Failed to initialize listeners:', error);
    }
  }

  /**
   * Setup native keyboard listeners for gameplay keys
   * Captures: Q, W, E, R, A, S, D, F, Z, X, C, V
   */
  private setupKeyboardListeners(): void {
    const gameKeys = ['q', 'w', 'e', 'r', 'a', 's', 'd', 'f', 'z', 'x', 'c', 'v', ' '];
    console.log('[TauriBridge] Keyboard listeners initialized for:', gameKeys);

    window.addEventListener('keydown', (event) => {
      if (!this.listenerActive) return;
      const key = event.key.toLowerCase();
      console.log('[TauriBridge] Keydown event:', key);

      if (key === ' ') {
        // Global SPACE key stop
        console.log('[TauriBridge] SPACE key detected - global stop');
        event.preventDefault();
        this.ngZone.run(() => {
          this.onGlobalStop.next();
        });
      } else if (gameKeys.includes(key)) {
        // Game key pressed
        console.log('[TauriBridge] Game key detected:', key.toUpperCase());
        event.preventDefault();
        this.ngZone.run(() => {
          this.onKeyTriggered.next(key.toUpperCase());
        });
      }
    });
  }

  // ========================================================================
  // FILE OPERATIONS
  // ========================================================================

  /**
   * Get all audio files from the harbor directory
   */
  async getHarborFiles(): Promise<string[]> {
    try {
      await this.waitForReady();
      const files = (await this.invoke('get_harbor_files')) as string[];
      console.log('[TauriBridge] Harbor files loaded:', files.length);
      return files;
    } catch (error) {
      console.error('[TauriBridge] Failed to get harbor files:', error);
      return [];
    }
  }

  /**
   * Open the audio folder in file explorer/finder
   */
  async openAudioFolder(): Promise<void> {
    try {
      await this.waitForReady();
      await this.invoke('open_audio_folder');
      console.log('[TauriBridge] Audio folder opened');
    } catch (error) {
      console.error('[TauriBridge] Failed to open audio folder:', error);
    }
  }

  /**
   * Open native file dialog to pick an audio file
   */
  async selectFile(): Promise<string | null> {
    try {
      await this.waitForReady();
      return await this.invoke('select_file');
    } catch (error) {
      if (error === 'User cancelled') return null;
      console.error('[TauriBridge] Failed to select file:', error);
      return null;
    }
  }

  /**
   * Toggle the developer tools window
   */
  async toggleDevTools(): Promise<void> {
    try {
      await this.waitForReady();
      await this.invoke('toggle_devtools');
    } catch (error) {
      console.error('[TauriBridge] Failed to toggle DevTools:', error);
    }
  }

  /**
   * Get the root path for the audio harbor from Rust
   */
  async getHarborPath(): Promise<string> {
    try {
      await this.waitForReady();
      return await this.invoke('get_harbor_path');
    } catch (error) {
      console.error('[TauriBridge] Failed to get harbor path:', error);
      return '';
    }
  }

  /**
   * Get the local file path for a resource
   */
  async getFilePath(relativePath: string): Promise<string> {
    const harbor = await this.getHarborPath();
    if (!harbor) return relativePath;
    return `${harbor}/${relativePath}`;
  }

  /**
   * Fetch audio file data from harbor
   * Converts binary data to ObjectURL for Web Audio API
   */
  async loadAudioFile(fileName: string): Promise<ArrayBuffer> {
    try {
      await this.waitForReady();
      // Invoke Rust command to read the file and return as bytes
      const bytes = await this.invoke('get_audio_file', { fileName });
      console.log('[TauriBridge] Raw bytes response:', bytes, 'Type:', typeof bytes);

      // Tauri returns binary data as a regular array or object, convert to Uint8Array
      let uint8Data: Uint8Array;
      if (bytes instanceof Uint8Array) {
        uint8Data = bytes;
      } else if (Array.isArray(bytes)) {
        uint8Data = new Uint8Array(bytes);
      } else if (bytes && typeof bytes === 'object') {
        // Handle case where it's a plain object with numeric keys
        const values = Object.values(bytes as Record<string, number>);
        uint8Data = new Uint8Array(values as number[]);
      } else {
        throw new Error(`Unexpected bytes format: ${typeof bytes}`);
      }

      // Create a proper ArrayBuffer from the Uint8Array
      return uint8Data.buffer.slice(uint8Data.byteOffset, uint8Data.byteOffset + uint8Data.byteLength) as ArrayBuffer;
    } catch (error) {
      console.error('[TauriBridge] Failed to load audio file:', error);
      throw error;
    }
  }

  // ========================================================================
  // AUDIO ENGINE CONTROL
  // ========================================================================

  /**
   * Load and decode an audio file in the Rust engine
   * @returns Duration and downsampled waveform of the loaded file
   */
  async audioLoad(key: string, path: string): Promise<{ duration: number, bpm: number, waveform: number[] }> {
    try {
      await this.waitForReady();
      return await this.invoke('audio_load', { key, path });
    } catch (error) {
      console.error(`[TauriBridge] Failed to load audio ${key}:`, error);
      throw error;
    }
  }

  /**
   * Play a sound from the Rust engine
   */
  async audioPlay(
    key: string,
    params: {
      volume: number;
      attack: number;
      release: number;
      looping: boolean;
      startTime: number;
      endTime: number;
      sync: boolean;
      sample_bpm: number;
    }
  ): Promise<void> {
    try {
      await this.waitForReady();
      await this.invoke('audio_play', {
        key,
        params: {
          volume: params.volume,
          attack: params.attack,
          release: params.release,
          looping: params.looping,
          startTime: params.startTime,
          endTime: params.endTime,
          sync: params.sync,
          sampleBpm: params.sample_bpm,
        }
      });
    } catch (error) {
      console.error(`[TauriBridge] Failed to play audio ${key}:`, error);
    }
  }

  /**
   * Update parameters for an active sound in the Rust engine
   */
  async audioUpdateParams(
    key: string,
    params: {
      volume: number;
      attack: number;
      release: number;
      looping: boolean;
      startTime: number;
      endTime: number;
      sync: boolean;
      sample_bpm: number;
    }
  ): Promise<void> {
    try {
      await this.waitForReady();
      await this.invoke('audio_update_params', {
        key,
        params: {
          volume: params.volume,
          attack: params.attack,
          release: params.release,
          looping: params.looping,
          startTime: params.startTime,
          endTime: params.endTime,
          sync: params.sync,
          sampleBpm: params.sample_bpm,
        }
      });
    } catch (error) {
      console.error(`[TauriBridge] Failed to update audio params ${key}:`, error);
    }
  }

  /**
   * Stop a sound in the Rust engine (with fade-out)
   */
  async audioStop(key: string, effective_release?: number): Promise<void> {
    try {
      await this.waitForReady();
      await this.invoke('audio_stop', { key, effective_release: effective_release ?? null });
    } catch (error) {
      console.error(`[TauriBridge] Failed to stop audio ${key}:`, error);
    }
  }

  /**
   * Set the global master BPM in the Rust engine
   */
  async audioSetMasterBpm(bpm: number): Promise<void> {
    try {
      await this.waitForReady();
      await this.invoke('audio_set_master_bpm', { bpm });
    } catch (error) {
      console.error('[TauriBridge] Failed to set master BPM:', error);
    }
  }

  /**
   * Get real-time audio levels and snapshots for all active sounds
   */
  async audioGetLevels(): Promise<LevelsResponse> {
    try {
      await this.waitForReady();
      return await this.invoke('audio_get_levels');
    } catch (error) {
      console.error('[TauriBridge] Failed to get audio levels:', error);
      return { data: {}, active_keys: [] };
    }
  }

  // ========================================================================
  // KEYBOARD CONTROL
  // ========================================================================

  /**
   * Toggle global keyboard listener on/off
   */
  async toggleListener(state: boolean): Promise<void> {
    try {
      // apply local state immediately so frontend listeners respect capture toggle
      this.listenerActive = !!state;
      await this.waitForReady();
      await this.invoke('toggle_listener', { state });
      console.log(`[TauriBridge] Keyboard listener: ${state ? 'ACTIVE' : 'RELEASED'}`);
    } catch (error) {
      console.error('[TauriBridge] Failed to toggle listener:', error);
    }
  }

  // ========================================================================
  // CONFIGURATION
  // ========================================================================

  /**
   * Apply configuration changes
   */
  async applyConfig(config: AppConfig): Promise<void> {
    try {
      await this.waitForReady();
      await this.invoke('apply_config', {
        config: {
          accent_color: config.accentColor,
          master_volume: config.masterVolume,
        },
      });
      console.log('[TauriBridge] Config applied:', config);
    } catch (error) {
      console.error('[TauriBridge] Failed to apply config:', error);
    }
  }

  // ========================================================================
  // LIFECYCLE
  // ========================================================================

  /**
   * Cleanup listeners on service destroy
   */
  ngOnDestroy(): void {
    this.listeners.forEach((unlisten) => {
      unlisten?.();
    });
  }
}
