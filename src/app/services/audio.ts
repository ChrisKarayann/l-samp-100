/**
 * audio.ts (service)
 * L-SAMP 100 | The Audio Engine
 * Purpose: Signal Chain Management & Backend State Synchronization
 */

import { Injectable } from '@angular/core';
import { Subject } from 'rxjs';
import { TauriBridgeService, VisualData } from './tauri-bridge.service';

@Injectable({
  providedIn: 'root',
})
export class Audio {
  private tauriBridge: TauriBridgeService;

  // Storage Maps (Metadata only, Audio is in Rust)
  private soundInfo = new Map<string, { duration: number, waveform: number[], bpm: number }>();
  private syncSettings = new Map<string, boolean>();
  private activePads = new Set<string>();
  private latestLevels: Record<string, VisualData> = {};

  // These are now purely in-memory. Persistence is handled by App.ts metadata.
  private fadeSettings = new Map<string, { attack: number, release: number }>();
  private gainSettings = new Map<string, number>();
  private loopSettings = new Map<string, boolean>();
  private masterBpm: number = 120;

  // Navigation Maps
  private trimInSettings = new Map<string, number>();
  private trimOutSettings = new Map<string, number>();

  private startTimes = new Map<string, number>();
  private stoppingPads = new Map<string, { fadeStartTime: number, releaseDuration: number }>();
  private loopToOneshotTransitions = new Map<string, number>(); // Tracks adjusted start time when loop is toggled off

  // RxJS Subjects for reactive event emission (zoneless-compatible)
  public padFinished$ = new Subject<string>();
  public fadeOutComplete$ = new Subject<string>();
  public loadingProgress$ = new Subject<{ key: string, progress: number }>();

  public onGlobalStop = new Subject<void>();
  public levelData$ = new Subject<Record<string, VisualData>>();
  private levelPollingActive = false;

  constructor(tauriBridge: TauriBridgeService) {
    this.tauriBridge = tauriBridge;

    // Listen for global stop from bridge
    this.tauriBridge.onGlobalStop.subscribe(() => {
      this.activePads.clear();
      this.onGlobalStop.next();
    });

    this.startLevelPolling();
  }

  private startLevelPolling() {
    if (this.levelPollingActive) return;
    this.levelPollingActive = true;

    const poll = async () => {
      if (!this.levelPollingActive) return;

      // --- THE POLLING BRAKE ---
      // Only bother the Rust engine if we know pads are active.
      // If empty, we wait a bit longer before checking again (Lower Frequency).
      if (this.activePads.size > 0) {
        const response = await this.tauriBridge.audioGetLevels();
        this.latestLevels = response.data;
        this.levelData$.next(response.data);

        const backendActive = new Set(response.active_keys);

        // Sync logic remains the same
        this.activePads.forEach((key) => {
          if (!backendActive.has(key)) {
            this.activePads.delete(key);
            this.stoppingPads.delete(key);
            this.padFinished$.next(key);
          }
        });

        backendActive.forEach(key => {
          if (!this.activePads.has(key)) {
            this.activePads.add(key);
          }
        });

        // While active, use High-Intensity (60fps) for smooth visualizers
        requestAnimationFrame(poll);
      } else {
        // If silent, check again in 100ms (Low-Intensity/Idle)
        // This saves massive IPC overhead when the app is just sitting there.
        setTimeout(() => poll(), 500);
      }
    };
    poll();
  }

  // REPLACED BLOCK FOR OPTIMIZATION
  /*
  private startLevelPolling() {
    if (this.levelPollingActive) return;
    this.levelPollingActive = true;

    const poll = async () => {
      if (!this.levelPollingActive) return;

      const response = await this.tauriBridge.audioGetLevels();
      this.latestLevels = response.data;
      this.levelData$.next(response.data);

      const backendActive = new Set(response.active_keys);

      // Sync activePads based on backend truth
      // If a pad is in activePads but NOT in backendActive, it has finished in Rust
      this.activePads.forEach((key) => {
        if (!backendActive.has(key)) {
          this.activePads.delete(key);
          this.stoppingPads.delete(key); // Cleanup
          this.padFinished$.next(key);
        }
      });

      // We could also add keys that are in backendActive but not in activePads
      // to handle cases where voices are triggered outside of toggleSound (e.g. MIDI)
      backendActive.forEach(key => {
        if (!this.activePads.has(key)) {
          this.activePads.add(key);
        }
      });

      requestAnimationFrame(poll);
    };
    poll();
  }
    */
  // END REPLACED BLOCK FOR OPTIMIZATION

  // --- 1. THE EYE (ANALYSIS) ---
  getLevel(key: string): number {
    return this.latestLevels[key]?.peak || 0;
  }

  getSamples(key: string): number[] {
    return this.latestLevels[key]?.samples || [];
  }

  // --- 2. LOADING & BUFFERING ---

  // REPLACED BLOCK FOR OPTIMIZATION
  /*
  async loadSound(key: string, filePath: string): Promise<boolean> {
    try {
      this.loadingProgress$.next({ key, progress: 0 });

      let finalPath = filePath;
      if (filePath.startsWith('music-app://harbor/')) {
        const fileName = filePath.substring('music-app://harbor/'.length);
        finalPath = await this.tauriBridge.getFilePath(fileName);
      } else if (filePath.startsWith('music-app://system')) {
        // Add a small yield to let UI render 'LOAD 0%'
        await new Promise(resolve => setTimeout(resolve, 50));
        const url = new URL(filePath);
        finalPath = url.searchParams.get('path') || filePath;
      }

      this.loadingProgress$.next({ key, progress: 20 });
      const loadResult = await this.tauriBridge.audioLoad(key, finalPath);
      console.log(`[AudioService] Raw LoadResult for ${key}:`, loadResult);

      const { duration, bpm, waveform } = loadResult;
      this.loadingProgress$.next({ key, progress: 100 });

      this.soundInfo.set(key, { duration, bpm, waveform });

      // HYDRATION: If no custom out-point is saved, initialize to the full duration
      if (!this.trimOutSettings.has(key)) {
        this.trimOutSettings.set(key, duration);
      }

      console.log(`[AudioService] ${key} buffered: ${duration}s, Detected BPM: ${bpm}`);
      setTimeout(() => this.loadingProgress$.next({ key, progress: -1 }), 500);
      return true;
    } catch (err) {
      console.error(`[RustAudio] Load failed:`, err);
      this.loadingProgress$.next({ key, progress: -1 });
      return false;
    }
  }
  */

  /**
 * Updated loadSound to support cached BPM from persistence
 */
  async loadSound(key: string, filePath: string, cachedBpm?: number): Promise<boolean> {
    try {
      this.loadingProgress$.next({ key, progress: 0 });

      let finalPath = filePath;
      if (filePath.startsWith('music-app://harbor/')) {
        const fileName = filePath.substring('music-app://harbor/'.length);
        finalPath = await this.tauriBridge.getFilePath(fileName);
      } else if (filePath.startsWith('music-app://system')) {
        await new Promise(resolve => setTimeout(resolve, 50));
        const url = new URL(filePath);
        finalPath = url.searchParams.get('path') || filePath;
      }

      this.loadingProgress$.next({ key, progress: 20 });

      // --- THE FIX: Pass the cachedBpm through to the bridge ---
      const loadResult = await this.tauriBridge.audioLoad(key, finalPath, cachedBpm);

      console.log(`[AudioService] Raw LoadResult for ${key}:`, loadResult);

      const { duration, bpm, waveform } = loadResult;
      this.loadingProgress$.next({ key, progress: 100 });

      this.soundInfo.set(key, { duration, bpm, waveform });

      if (!this.trimOutSettings.has(key)) {
        this.trimOutSettings.set(key, duration);
      }

      console.log(`[AudioService] ${key} buffered: ${duration}s, BPM: ${bpm} ${cachedBpm ? '(from cache)' : '(analyzed)'}`);

      setTimeout(() => this.loadingProgress$.next({ key, progress: -1 }), 500);
      return true;
    } catch (err) {
      console.error(`[RustAudio] Load failed:`, err);
      this.loadingProgress$.next({ key, progress: -1 });
      return false;
    }
  }

  hasBuffer(key: string): boolean {
    return this.soundInfo.has(key);
  }

  getWaveform(key: string): number[] {
    return this.soundInfo.get(key)?.waveform || [];
  }

  getDuration(key: string): number {
    return this.soundInfo.get(key)?.duration || 0;
  }

  // --- 3. THE TRIGGER ENGINE ---
  async toggleSound(key: string): Promise<boolean> {
    if (this.activePads.has(key)) {
      this.stopSoundWithFade(key);
      return false;
    }

    if (!this.soundInfo.has(key)) return false;

    // --- NAVIGATION CALCULATION ---
    const { in: startOffset, out: endOffset } = this.getTrimParams(key);
    const targetVol = this.getGain(key);
    const { attack, release } = this.getFadeParams(key);
    const looping = this.getLoopState(key);

    // Add to active pads immediately for UI feedback
    this.activePads.add(key);
    this.startTimes.set(key, Date.now() / 1000);
    this.stoppingPads.delete(key);
    this.loopToOneshotTransitions.delete(key); // Clear any previous transition state

    const sync = this.getSyncState(key);
    const sample_bpm = this.getBpm(key);

    await this.tauriBridge.audioPlay(key, {
      volume: targetVol,
      attack,
      release,
      looping,
      startTime: startOffset,
      endTime: endOffset,
      sync,
      sample_bpm,
    });

    return true;
  }

  // --- 4. FADE & VOLUME CONTROL ---
  private stopSoundWithFade(key: string): void {
    if (this.activePads.has(key) && !this.stoppingPads.has(key)) {
      const { attack, release } = this.getFadeParams(key);
      const startTime = this.startTimes.get(key) || 0;
      const now = Date.now() / 1000;
      const elapsed = now - startTime;

      let effectiveRelease = release;

      // SYMMETRY: Clamp fade-out if toggled during fade-in
      if (elapsed < attack) {
        effectiveRelease = elapsed;
      }

      // BOUNDARY PROTECTION: Clamp fade-out to remaining time until mark-out
      const { in: startPoint, out: endPoint } = this.getTrimParams(key);
      const originalSliceDuration = Math.max(0, endPoint - startPoint);

      // BPM Warp Calculation
      let effectiveSliceDuration = originalSliceDuration;
      if (this.getSyncState(key)) {
        const sampleBpm = this.getBpm(key);
        if (sampleBpm > 0 && this.masterBpm > 0) {
          effectiveSliceDuration = originalSliceDuration * (sampleBpm / this.masterBpm);
        }
      }

      // Calculate remaining time in current iteration (works for both loop and one-shot)
      const isLooping = this.getLoopState(key);
      const positionInSlice = isLooping ? (elapsed % effectiveSliceDuration) : elapsed;
      const remainingTime = effectiveSliceDuration - positionInSlice;

      if (remainingTime > 0 && remainingTime < effectiveRelease) {
        effectiveRelease = remainingTime;
      }

      this.stoppingPads.set(key, {
        fadeStartTime: now,
        releaseDuration: effectiveRelease
      });

      this.loopToOneshotTransitions.delete(key); // Clear transition tracking on stop
      this.tauriBridge.audioStop(key, effectiveRelease); // Pass effective release to Rust
      // activePads removal is now handled by the polling loop's backend sync
    }
  }

  private async syncParamsToRust(key: string) {
    if (this.activePads.has(key)) {
      const { in: startOffset, out: endOffset } = this.getTrimParams(key);
      const targetVol = this.getGain(key);
      const { attack, release } = this.getFadeParams(key);
      const looping = this.getLoopState(key);

      const sync = this.getSyncState(key);
      const sample_bpm = this.getBpm(key);

      await this.tauriBridge.audioUpdateParams(key, {
        volume: targetVol,
        attack,
        release,
        looping,
        startTime: startOffset,
        endTime: endOffset,
        sync,
        sample_bpm,
      });
    }
  }

  async stopAllSounds(keysOverride?: string[]) {
    const keysToStop = keysOverride || Array.from(this.activePads);

    for (const key of keysToStop) {
      const { release } = this.getFadeParams(key);
      // We send the stop, but we don't necessarily need to track 
      // the fade-out if the user wanted a GLOBAL STOP.
      await this.tauriBridge.audioStop(key.toUpperCase(), release);
    }

    // Clear everything internal
    this.activePads.clear();
    this.stoppingPads.clear(); // <--- Clear the "ghost" timers
    this.loopToOneshotTransitions.clear();
  }

  // --- 5. PARAMETER CALIBRATION ---
  setFadeParams(key: string, attack: number, release: number) {
    this.fadeSettings.set(key, { attack, release });
  }

  getFadeParams(key: string) {
    return this.fadeSettings.get(key) || { attack: 0.1, release: 0.1 };
  }

  getEffectiveFadeOut(key: string): number | undefined {
    // Returns the actual fade-out duration being used (after boundary clamping)
    return this.stoppingPads.get(key)?.releaseDuration;
  }

  setGain(key: string, value: number) {
    this.gainSettings.set(key, value);
    this.syncParamsToRust(key);
  }

  getGain(key: string): number {
    return this.gainSettings.get(key) ?? 0.8;
  }

  setMasterVolume(val: number) {
    this.tauriBridge.applyConfig({
      accentColor: '', // Handle elsewhere
      masterVolume: val
    });
  }

  setMasterBpm(val: number) {
    this.masterBpm = val;
    this.tauriBridge.audioSetMasterBpm(val);
  }

  setSyncState(key: string, state: boolean) {
    this.syncSettings.set(key, state);
    this.syncParamsToRust(key);
  }

  getSyncState(key: string): boolean {
    return this.syncSettings.get(key) ?? false;
  }

  getBpm(key: string): number {
    const info = this.soundInfo.get(key);
    const bpm = info?.bpm ?? 120;
    console.log(`[AudioService] getBpm(${key}) -> ${bpm} (exists: ${!!info})`);
    return bpm;
  }

  setBpm(key: string, val: number) {
    const info = this.soundInfo.get(key);
    if (info) {
      info.bpm = val;
      this.syncParamsToRust(key);
    }
  }

  getRemainingTime(key: string): number {
    const info = this.soundInfo.get(key);
    let startTime = this.startTimes.get(key);
    const { in: startPoint, out: endPoint } = this.getTrimParams(key);

    if (!info || startTime === undefined || !this.activePads.has(key)) {
      return 0;
    }

    const originalSliceDuration = Math.max(0, endPoint - startPoint);

    // BPM Warp Calculation
    let effectiveSliceDuration = originalSliceDuration;
    if (this.getSyncState(key)) {
      const sampleBpm = this.getBpm(key);
      if (sampleBpm > 0 && this.masterBpm > 0) {
        effectiveSliceDuration = originalSliceDuration * (sampleBpm / this.masterBpm);
      }
    }

    // Check if this pad transitioned from loop to oneshot mid-playback
    const transitionStartTime = this.loopToOneshotTransitions.get(key);
    if (transitionStartTime !== undefined) {
      startTime = transitionStartTime;
    }

    const now = Date.now() / 1000;
    const elapsedSinceTrigger = now - startTime;

    if (this.getLoopState(key)) {
      const loopElapsed = elapsedSinceTrigger % effectiveSliceDuration;
      return effectiveSliceDuration - loopElapsed;
    }

    const remaining = effectiveSliceDuration - elapsedSinceTrigger;
    return remaining > 0 ? remaining : 0;
  }

  setLoopState(key: string, state: boolean) {
    const wasLooping = this.loopSettings.get(key) ?? false;
    this.loopSettings.set(key, state);

    // If toggling FROM loop TO oneshot while playing, capture position and clamp release
    if (wasLooping && !state && this.activePads.has(key)) {
      const startTime = this.startTimes.get(key);
      const remainingInLoop = this.getRemainingTime(key);

      // Get parameters from your actual Maps
      const { in: startPoint, out: endPoint } = this.getTrimParams(key);
      const { attack, release: targetRelease } = this.getFadeParams(key);

      // THE CLAMP: Ensure fade doesn't outlive the sample iteration
      const clampedRelease = Math.min(remainingInLoop, targetRelease);

      if (startTime !== undefined) {
        const originalSliceDuration = Math.max(0, endPoint - startPoint);
        let effectiveSliceDuration = originalSliceDuration;

        // BPM Warp Calculation
        if (this.getSyncState(key)) {
          const sampleBpm = this.getBpm(key);
          if (sampleBpm > 0 && this.masterBpm > 0) {
            effectiveSliceDuration = originalSliceDuration * (sampleBpm / this.masterBpm);
          }
        }

        const elapsedSinceTrigger = (Date.now() / 1000) - startTime;
        const loopElapsed = elapsedSinceTrigger % effectiveSliceDuration;

        // Virtual start time for One-shot logic
        const adjustedStartTime = (Date.now() / 1000) - loopElapsed;
        this.loopToOneshotTransitions.set(key, adjustedStartTime);

        // --- SYNC TO RUST WITH CLAMPED RELEASE ---
        this.tauriBridge.audioUpdateParams(key, {
          volume: this.gainSettings.get(key) ?? 0.8,
          attack: attack,
          release: clampedRelease, // Use the smaller value (remaining time vs setting)
          looping: false,
          startTime: startPoint,
          endTime: endPoint,
          sync: this.getSyncState(key),
          sample_bpm: this.getBpm(key)
        });
      }
    } else if (state) {
      // Toggling back to loop: clear transition and sync normally
      this.loopToOneshotTransitions.delete(key);
      this.syncParamsToRust(key);
    } else {
      // Just a standard setting change while not playing
      this.syncParamsToRust(key);
    }
  }

  getLoopState(key: string): boolean {
    return this.loopSettings.get(key) ?? false;
  }

  // --- 6. NAVIGATION MODULE SUPPORT ---
  setTrimParams(key: string, trimIn: number, trimOut: number) {
    this.trimInSettings.set(key, trimIn);
    this.trimOutSettings.set(key, trimOut);
  }

  getTrimParams(key: string) {
    const info = this.soundInfo.get(key);

    const savedIn = this.trimInSettings.get(key) ?? 0;
    let savedOut = this.trimOutSettings.get(key);

    if (savedOut === undefined || savedOut === 0) {
      savedOut = (info ? info.duration : 0);
    }

    if (info && savedOut > info.duration) {
      savedOut = info.duration;
    }

    return { in: savedIn, out: savedOut };
  }

  getBuffer(key: string): { duration: number, waveform: number[] } | undefined {
    return this.soundInfo.get(key);
  }

  stopSound(key: string) {
    this.stopSoundWithFade(key);
  }

  unloadSound(key: string) {
    this.soundInfo.delete(key);
    this.trimInSettings.delete(key);
    this.trimOutSettings.delete(key);
  }
}
