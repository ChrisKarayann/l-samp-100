/**
 * app.ts (component) - ZONELESS REFACTOR
 * L-SAMP 100 | The Conductor
 * Purpose: Orchestrating Hardware, Audio Service, and the Visual Interface
 * 
 * MIGRATION: Now uses Angular signals for state management instead of zone.js
 */

import {
  Component,
  OnInit,
  OnDestroy,
  NgZone,
  ViewChild,
  ElementRef,
  signal,
  computed,
  effect,
  ChangeDetectionStrategy,
  ViewEncapsulation,
} from '@angular/core';
import { CommonModule } from '@angular/common';
import { FormsModule } from '@angular/forms';
import { Audio } from './services/audio';
import { TauriBridgeService } from './services/tauri-bridge.service';
import { Subscription } from 'rxjs';

@Component({
  selector: 'app-root',
  standalone: true,
  imports: [CommonModule, FormsModule],
  templateUrl: './app.html',
  styleUrl: './app.scss',
  encapsulation: ViewEncapsulation.None,
})
export class App implements OnInit, OnDestroy {
  // --- 1. CORE STATE & PHYSICAL MAPPING ---
  availablePads: { [key: string]: string[] } = {
    '1x4': ['Q', 'W', 'E', 'R'],
    '2x4': ['Q', 'W', 'E', 'R', 'A', 'S', 'D', 'F'],
    '3x4': ['Q', 'W', 'E', 'R', 'A', 'S', 'D', 'F', 'Z', 'X', 'C', 'V']
  };

  // Signals for mutable state
  pads = signal<string[]>([]);
  matrix = signal<'1x4' | '2x4' | '3x4'>('2x4');
  activeKey = signal<string | null>(null);
  selectedPad = signal<string | null>(null);
  playingPads = signal<Set<string>>(new Set());
  fadingPads = signal<Set<string>>(new Set());
  showMenuDropdown = signal<boolean>(false);

  loadingProgress = signal<Map<string, number>>(new Map());

  padNames = signal<Record<string, string>>({});
  padBpm = signal<Record<string, number>>({});
  padSync = signal<Record<string, boolean>>({});
  masterBpm = signal<number>(120);
  editingBpm = signal<string | null>(null);
  digitBuffer = signal<string>(''); // Pure digit string (max 4)
  fileTruth = signal<Record<string, string>>({});
  loadedPads = signal<Set<string>>(new Set());
  clockMode = signal<'manual' | 'auto'>('manual');
  harbourFiles = signal<string[]>([]);
  groupedFiles = computed(() => this.getGroupedFiles(this.harbourFiles()));

  isKeyboardCaptured = signal<boolean>(true);
  accentColor = signal<string>('#00ffcc');
  masterVolume = signal<number>(1.0);
  showModal = signal<boolean>(false);
  modalMode = signal<'info' | 'instructions' | 'settings' | 'factory-reset' | 'clear-all' | 'clear-selected' | null>(null);

  // Trim/Waveform parameters
  currentTrimIn = signal<number>(0);
  currentTrimOut = signal<number>(1);
  maxDuration = signal<number>(1);
  inMarkerPos = computed(() => (this.currentTrimIn() / this.maxDuration()) * 100);
  outMarkerPos = computed(() => (this.currentTrimOut() / this.maxDuration()) * 100);

  // Volume/Fade parameters
  currentVol = signal<number>(0.8);
  currentAttack = signal<number>(0.1);
  currentRelease = signal<number>(0.1);

  // SVG visualizations (computed from parameters)
  gainPath = computed(() => this.getGainPulsePath(this.currentVol()));
  envelopePath = computed(() => this.getDynamicEnvelopePath(this.currentAttack(), this.currentRelease()));

  // Timer display (computed from playing pads)
  displayRemainingTime = computed(() => {
    const result: Record<string, string> = {};
    const playing = this.playingPads();
    playing.forEach(key => {
      result[key] = this.formatTime(key);
    });
    return result;
  });

  showFinalResetConfirmation = signal<boolean>(false);

  private animationId: number = 0;
  private processingKeys = new Set<string>();
  private padTimers = new Map<string, number>();
  private subscriptions: Subscription[] = [];

  @ViewChild('controlBody') controlBody!: ElementRef;
  @ViewChild('waveformCanvas') canvasRef!: ElementRef<HTMLCanvasElement>;

  constructor(private audio: Audio, private zone: NgZone, private tauriBridge: TauriBridgeService) {
    // Setup effects for Audio service subscriptions
    effect(() => {
      const subscription = this.audio.padFinished$.subscribe((key: string) => {
        this.playingPads.update(set => {
          const newSet = new Set(set);
          newSet.delete(key);
          return newSet;
        });
        this.fadingPads.update(set => {
          const newSet = new Set(set);
          newSet.delete(key);
          return newSet;
        });
        this.padTimers.delete(key);
      });
      this.subscriptions.push(subscription);
      return () => subscription.unsubscribe();
    });

    effect(() => {
      const subscription = this.audio.fadeOutComplete$.subscribe((key: string) => {
        this.fadingPads.update(set => {
          const newSet = new Set(set);
          newSet.delete(key);
          return newSet;
        });
      });
      this.subscriptions.push(subscription);
      return () => subscription.unsubscribe();
    });

    effect(() => {
      const subscription = this.audio.loadingProgress$.subscribe(({ key, progress }) => {
        this.loadingProgress.update(map => {
          const newMap = new Map(map);
          if (progress === -1) {
            newMap.delete(key);
          } else {
            newMap.set(key, progress);
          }
          return newMap;
        });
      });
      this.subscriptions.push(subscription);
      return () => subscription.unsubscribe();
    });

    // Handle deferred waveform drawing when a pad finishes loading
    effect(() => {
      const selected = this.selectedPad();
      const loaded = this.loadedPads();
      if (selected && loaded.has(selected)) {
        // Run after current change detection to ensure Audio service state is reflected
        requestAnimationFrame(() => this.syncMarkerVariables());
      }
    });

    // DEBUG: Periodically log the state of the BPM signal
    setInterval(() => {
      const bpmState = this.padBpm();
      if (Object.keys(bpmState).length > 0) {
        console.log('[DEBUG] Pad BPM State:', JSON.stringify(bpmState));
      }
    }, 5000);

    // AUTO-BPM EFFECT: Sync master clock to average of loaded samples when in 'Auto' mode
    effect(() => {
      const mode = this.clockMode();
      const loaded = this.loadedPads();
      const bpms = this.padBpm();

      if (mode === 'auto' && loaded.size > 0) {
        let total = 0;
        let count = 0;
        loaded.forEach(key => {
          const b = bpms[key];
          if (b && b > 0) {
            total += b;
            count++;
          }
        });

        if (count > 0) {
          const avg = parseFloat((total / count).toFixed(1));
          if (this.masterBpm() !== avg) {
            this.updateMasterBpm(avg);
          }
        }
      }
    });
  }

  // --- 2. LIFECYCLE & INITIALIZATION ---
  async ngOnInit() {
    const savedColor = localStorage.getItem('lsamp_theme') || '#00ffcc';
    this.setTheme(savedColor);

    const savedMatrix = (localStorage.getItem('lsamp_matrix') as '1x4' | '2x4' | '3x4') || '2x4';
    this.matrix.set(savedMatrix);
    this.pads.set(this.availablePads[savedMatrix]);

    const savedMasterVol = localStorage.getItem('lsamp_master_vol');
    if (savedMasterVol) {
      const vol = parseFloat(savedMasterVol);
      this.masterVolume.set(vol);
      this.audio.setMasterVolume(vol);
    }

    const savedMasterBpm = localStorage.getItem('lsamp_master_bpm');
    if (savedMasterBpm) {
      this.updateMasterBpm(parseFloat(savedMasterBpm));
    }

    const savedClockMode = localStorage.getItem('lsamp_clock_mode') as 'manual' | 'auto';
    if (savedClockMode) {
      this.clockMode.set(savedClockMode);
    }

    this.pads().forEach(key => {
      this.loadPadMetadata(key);
    });

    await this.refreshLibrary();
    this.setupHardwareListeners();
    this.startVisualizer();
  }

  ngOnDestroy() {
    if (this.animationId) cancelAnimationFrame(this.animationId);
    this.subscriptions.forEach(sub => sub.unsubscribe());
  }

  // --- 3. HARDWARE & BRIDGE LISTENERS ---
  private setupHardwareListeners() {
    // Key triggered listener
    this.tauriBridge.onKeyTriggered.subscribe((key: string) => {
      const normalizedKey = key.toUpperCase();

      if (!this.pads().includes(normalizedKey)) return;

      if (this.processingKeys.has(normalizedKey)) return;
      this.processingKeys.add(normalizedKey);

      const wasPlaying = this.playingPads().has(normalizedKey);

      this.audio.toggleSound(normalizedKey).then(isNowPlaying => {

        // --- THE RE-WAKE LOGIC ---
        if (isNowPlaying && this.animationId === 0) {
          this.startVisualizer();
        }
        // -------------------------

        this.activeKey.set(normalizedKey);

        setTimeout(() => {
          this.activeKey.set(null);
          this.processingKeys.delete(normalizedKey);
        }, 200);

        if (isNowPlaying) {
          this.playingPads.update(set => {
            const newSet = new Set(set);
            newSet.add(normalizedKey);
            return newSet;
          });
          this.fadingPads.update(set => {
            const newSet = new Set(set);
            newSet.delete(normalizedKey);
            return newSet;
          });
        } else if (wasPlaying) {
          this.playingPads.update(set => {
            const newSet = new Set(set);
            newSet.delete(normalizedKey);
            return newSet;
          });
          this.fadingPads.update(set => {
            const newSet = new Set(set);
            newSet.add(normalizedKey);
            return newSet;
          });
        }
      });
    });

    // Global stop listener (SPACE key)
    this.tauriBridge.onGlobalStop.subscribe(async () => {
      const currentPlaying = Array.from(this.playingPads());
      const currentFading = Array.from(this.fadingPads());

      // Combine both to ensure nothing is left behind in Rust
      const allActive = [...new Set([...currentPlaying, ...currentFading])];

      await this.audio.stopAllSounds(allActive);

      // CRITICAL: Wipe both signals to trigger the "Final Sweep" in startVisualizer
      this.playingPads.set(new Set());
      this.fadingPads.set(new Set());
      this.activeKey.set(null);
    });

    // Modal opening
    this.tauriBridge.onOpenModal.subscribe((mode: 'instructions' | 'settings' | 'info' | 'factory-reset' | 'clear-selected' | 'clear-all') => {
      this.modalMode.set(mode);
      this.showModal.set(true);
    });

    // Menu events: Clear Selected Pad
    window.addEventListener('menu:clear-selected', () => {
      this.openModal('clear-selected');
    });

    // Menu events: Clear All Pads
    window.addEventListener('menu:clear-all', () => {
      this.openModal('clear-all');
    });
  }

  // --- 4. FILE & HARBOR MANAGEMENT ---
  async refreshLibrary() {
    const newFiles = await this.tauriBridge.getHarborFiles();
    this.harbourFiles.set(newFiles);

    if (this.selectedPad()) {
      this.syncMarkerVariables();
    }
  }

  openHarbor() {
    this.tauriBridge.openAudioFolder();
  }

  toggleMenuOpen() {
    this.showMenuDropdown.update(state => !state);
  }

  openModal(mode: 'instructions' | 'settings' | 'info' | 'factory-reset' | 'clear-selected' | 'clear-all') {
    this.showMenuDropdown.set(false);
    this.modalMode.set(mode);
    this.showModal.set(true);
  }

  loadFromHarbor(fileName: string, targetPad: string) {
    if (!fileName) {
      this.fileTruth.update(rec => {
        const nr = { ...rec };
        delete nr[targetPad];
        return nr;
      });
      this.padNames.update(rec => {
        const nr = { ...rec };
        delete nr[targetPad];
        return nr;
      });
      localStorage.removeItem(`lsamp_pad_meta_${targetPad}`);
      return;
    }

    if (document.activeElement instanceof HTMLElement) {
      document.activeElement.blur();
    }

    const harborUrl = `music-app://harbor/${fileName}`;

    // Clear ghost state if this pad is selected
    if (this.selectedPad() === targetPad) {
      this.maxDuration.set(0.001);
      this.currentTrimIn.set(0);
      this.currentTrimOut.set(0);
      this.inMarkerPos;
      this.outMarkerPos;
    }

    this.fileTruth.update(rec => ({ ...rec, [targetPad]: fileName }));
    this.padNames.update(rec => ({ ...rec, [targetPad]: this.getDisplayName(fileName) }));

    this.executeLoad(targetPad, harborUrl);
  }

  private async executeLoad(targetPad: string, url: string) {
    const success = await this.audio.loadSound(targetPad, url);
    if (success) {
      // Hydrate BPM from backend detection
      const detectedBpm = this.audio.getBpm(targetPad);
      console.log(`[App] Hydrating ${targetPad} with BPM: ${detectedBpm}`);

      if (detectedBpm > 0) {
        this.updatePadBpm(targetPad, detectedBpm);
      }

      if (this.selectedPad() === targetPad) {
        requestAnimationFrame(() => this.syncMarkerVariables());
      }
      this.loadedPads.update(set => {
        const n = new Set(set);
        n.add(targetPad);
        return n;
      });
    }
  }

  async triggerExternalFileSelection(key: string) {
    const realPath = await this.tauriBridge.selectFile();
    if (realPath) {
      const fileName = realPath.split(/[/\\]/).pop() || 'Unknown File';
      this.fileTruth.update(rec => ({ ...rec, [key]: realPath }));
      this.padNames.update(rec => ({ ...rec, [key]: fileName }));

      const url = `music-app://system?path=${encodeURIComponent(realPath)}`;
      this.executeLoad(key, url);
    }
  }

  // --- UTILS & DEV ---
  reloadUI() {
    window.location.reload();
  }

  toggleDevTools() {
    this.tauriBridge.toggleDevTools();
  }

  handleLabelFocus(key: string, event: FocusEvent) {
    // Clear the name in the signal so the input actually empties
    this.updateCustomName(key, '');
  }

  handleLabelBlur(key: string, event: FocusEvent) {
    const input = event.target as HTMLInputElement;
    const newVal = input.value.trim();

    if (newVal === '') {
      // Revert to file name if available, otherwise stay empty (placeholder)
      const filePath = this.fileTruth()[key];
      if (filePath) {
        this.updateCustomName(key, this.getDisplayName(filePath));
      } else {
        this.updateCustomName(key, '');
      }
    } else {
      this.updateCustomName(key, newVal);
    }
    this.savePadMetadata(key);
  }

  updateCustomName(key: string, newName: string) {
    this.padNames.update(rec => ({ ...rec, [key]: newName }));
    this.savePadMetadata(key);
  }

  updatePadBpm(key: string, bpmVal: string | number) {
    const bpm = typeof bpmVal === 'string' ? parseFloat(bpmVal) : bpmVal;
    if (!isNaN(bpm) && bpm >= 0) {
      console.log(`[App] updatePadBpm(${key}, ${bpm}) - Updating record`);
      this.padBpm.update(rec => ({ ...rec, [key]: bpm }));
      this.audio.setBpm(key, bpm); // Propagate to audio service
      this.savePadMetadata(key);
    }
  }

  toggleSync(key: string) {
    const currentState = this.padSync()[key] || false;
    const newState = !currentState;
    this.padSync.update(rec => ({ ...rec, [key]: newState }));
    this.audio.setSyncState(key, newState);
    this.savePadMetadata(key);
  }

  updateMasterBpm(val: number) {
    this.masterBpm.set(val);
    this.audio.setMasterBpm(val);
    localStorage.setItem('lsamp_master_bpm', val.toString());
  }

  setClockMode(mode: 'manual' | 'auto') {
    this.clockMode.set(mode);
    localStorage.setItem('lsamp_clock_mode', mode);
    if (mode === 'manual') {
      this.updateMasterBpm(120);
    }
  }

  getBpmParts(key: string) {
    let val = 0;
    if (this.editingBpm() === key && this.digitBuffer()) {
      val = parseInt(this.digitBuffer()) / 10;
    } else {
      val = this.padBpm()[key] || 0;
    }
    const parts = val.toFixed(1).split('.');
    return { integer: parts[0], decimal: parts[1] };
  }

  // --- 5. AUDIO PARAMETER CALIBRATION ---
  updateVolume(event: Event) {
    if (this.selectedPad()) {
      const target = event.target as HTMLInputElement;
      const val = parseFloat(target.value);
      this.currentVol.set(val);
      this.audio.setGain(this.selectedPad()!, val);
      this.savePadMetadata(this.selectedPad()!);
    }
  }

  updateFade(event: Event, type: 'attack' | 'release') {
    if (this.selectedPad()) {
      const target = event.target as HTMLInputElement;
      const val = parseFloat(target.value);
      if (type === 'attack') {
        this.currentAttack.set(val);
      } else {
        this.currentRelease.set(val);
      }
      this.audio.setFadeParams(this.selectedPad()!, this.currentAttack(), this.currentRelease());
      this.savePadMetadata(this.selectedPad()!);
    }
  }

  updateGlobalGain(value: string | number) {
    const val = typeof value === 'string' ? parseFloat(value) : value;
    this.masterVolume.set(val);
    this.audio.setMasterVolume(val);
    localStorage.setItem('lsamp_master_vol', val.toString());
  }

  // --- 6. VISUAL FEEDBACK & THE EYE ---
  // REPLACE THIS WITH THE BLOCK BELOW FOR PERFORMANCE OPTIMIZATION
  /*
  startVisualizer() {
    // Keep canvas rendering outside Angular's change detection for performance
    this.zone.runOutsideAngular(() => {
      const draw = () => {
        this.pads().forEach(key => {
          const canvas = document.getElementById(`osc-${key}`) as HTMLCanvasElement;
          const ctx = canvas?.getContext('2d');
          if (!ctx) return;

          const peak = this.audio.getLevel(key);
          const samples = this.audio.getSamples(key);
          const isPlaying = this.playingPads().has(key);

          ctx.clearRect(0, 0, canvas.width, canvas.height);
          ctx.lineWidth = 2;
          ctx.strokeStyle = isPlaying ? this.accentColor() : 'rgba(255,255,255,0.1)';

          if (this.isPadActive(key)) {
            const remainingTime = this.audio.getRemainingTime(key);

            const timerEl = document.getElementById(`timer-${key}`);
            if (timerEl) {
              timerEl.innerText = this.formatTime(key, remainingTime);
            }
          }

          // Draw a real time-domain oscilloscope "thread"
          ctx.beginPath();
          const midY = canvas.height / 2;
          const width = canvas.width;

          if (isPlaying && samples.length > 0) {
            const sliceWidth = width / (samples.length - 1);
            for (let i = 0; i < samples.length; i++) {
              const x = i * sliceWidth;
              const v = samples[i] * 0.8; // Scale for visibility
              const y = midY + (v * midY);

              if (i === 0) ctx.moveTo(x, y);
              else ctx.lineTo(x, y);
            }
          } else {
            ctx.moveTo(0, midY);
            ctx.lineTo(width, midY);
          }

          ctx.stroke();
        });
        this.animationId = requestAnimationFrame(draw);
      };
      draw();
    });
  }
  */
  // END OF BLOCK TO REPLACE

  startVisualizer() {
    // 1. Prevent multiple loops from running simultaneously
    if (this.animationId) return;

    this.zone.runOutsideAngular(() => {
      const draw = () => {
        const activeKeys = this.playingPads();
        const fadingKeys = this.fadingPads();

        // 2. THE GATEKEEPER: If no pads are active or fading, kill the loop to save 30% CPU
        // THE REFINED GATEKEEPER
        if (activeKeys.size === 0 && fadingKeys.size === 0) {
          // Before we kill the loop, we do one last cleanup of the UI
          this.pads().forEach(key => {
            const canvas = document.getElementById(`osc-${key}`) as HTMLCanvasElement;
            const ctx = canvas?.getContext('2d');
            if (ctx) ctx.clearRect(0, 0, canvas.width, canvas.height);

            const timerEl = document.getElementById(`timer-${key}`);
            if (timerEl) timerEl.innerText = '00:00.0';
          });

          this.animationId = 0; // The Eye closes, but the room is clean
          return;
        }

        this.pads().forEach(key => {
          const isCurrentlyActive = activeKeys.has(key) || fadingKeys.has(key);

          // Timer Update Logic
          const timerEl = document.getElementById(`timer-${key}`);
          if (timerEl) {
            if (isCurrentlyActive) {
              const remainingTime = this.audio.getRemainingTime(key);
              timerEl.innerText = this.formatTime(key, remainingTime);
            } else {
              // Reset timer display if pad is dead
              timerEl.innerText = '00:00.0';
            }
          }

          // Oscilloscope Drawing Logic
          const canvas = document.getElementById(`osc-${key}`) as HTMLCanvasElement;
          const ctx = canvas?.getContext('2d');
          if (!ctx) return;

          if (isCurrentlyActive) {
            const samples = this.audio.getSamples(key);
            const isPlaying = activeKeys.has(key);

            ctx.clearRect(0, 0, canvas.width, canvas.height);
            ctx.lineWidth = 2;
            ctx.strokeStyle = isPlaying ? this.accentColor() : 'rgba(255,255,255,0.1)';

            ctx.beginPath();
            const midY = canvas.height / 2;
            const width = canvas.width;

            if (isPlaying && samples.length > 0) {
              const sliceWidth = width / (samples.length - 1);
              for (let i = 0; i < samples.length; i++) {
                const x = i * sliceWidth;
                const v = samples[i] * 0.8;
                const y = midY + (v * midY);
                if (i === 0) ctx.moveTo(x, y);
                else ctx.lineTo(x, y);
              }
            } else {
              ctx.moveTo(0, midY);
              ctx.lineTo(width, midY);
            }
            ctx.stroke();
          } else {
            // If the pad just stopped, clear its canvas one last time
            ctx.clearRect(0, 0, canvas.width, canvas.height);
          }
        });

        this.animationId = requestAnimationFrame(draw);
      };

      // Start the first frame
      this.animationId = requestAnimationFrame(draw);
    });
  }


  getDynamicEnvelopePath(attack: number, release: number): string {
    if (!this.selectedPad()) return 'M 0 12 L 10 2 L 30 2 L 40 12';
    const attackX = (attack / 4) * 15;
    const releaseX = 40 - (release / 4) * 15;
    return `M 0 12 L ${attackX} 2 L ${releaseX} 2 L 40 12`;
  }

  formatTime(key: string, providedTime?: number): string {
    const totalSeconds = providedTime !== undefined ? providedTime : this.audio.getRemainingTime(key);
    if (totalSeconds <= 0) return '00:00.0';
    const mm = Math.floor(totalSeconds / 60).toString().padStart(2, '0');
    const ss = Math.floor(totalSeconds % 60).toString().padStart(2, '0');
    const t = Math.floor((totalSeconds % 1) * 10);
    return `${mm}:${ss}.${t}`;
  }

  // --- 7. UTILITIES ---
  setTheme(color: string) {
    this.accentColor.set(color);
    localStorage.setItem('lsamp_theme', color);
    const rgbMap: { [key: string]: string } = {
      '#00ffcc': '0, 255, 204',
      '#ffbf00': '255, 191, 0',
      '#ff4d4d': '255, 77, 77'
    };
    const rgbValue = rgbMap[color] || '0, 255, 204';
    document.documentElement.style.setProperty('--accent-rgb', rgbValue);
    document.documentElement.style.setProperty('--accent-color', color);

    if (this.selectedPad()) {
      requestAnimationFrame(() => this.drawWaveform());
    }
  }

  updateMatrix(size: '1x4' | '2x4' | '3x4') {
    this.matrix.set(size);
    this.pads.set(this.availablePads[size]);
    localStorage.setItem('lsamp_matrix', size);
    this.pads().forEach(key => {
      if (!this.fileTruth()[key]) {
        this.loadPadMetadata(key);
      }
    });
    this.startVisualizer();
  }

  getTransitionDuration(key: string): string {
    if (this.playingPads().has(key)) {
      const { attack } = this.audio.getFadeParams(key);
      return attack > 0 ? `${attack}s` : '0.1s';
    } else if (this.fadingPads().has(key)) {
      // Use the effective fade-out duration (after boundary clamping)
      const effectiveRelease = this.audio.getEffectiveFadeOut(key);
      if (effectiveRelease !== undefined) {
        return effectiveRelease > 0 ? `${effectiveRelease}s` : '0.1s';
      }
      // Fallback to raw release if not in stoppingPads yet
      const { release } = this.audio.getFadeParams(key);
      return release > 0 ? `${release}s` : '0.1s';
    }
    return '0.4s';
  }

  toggleKeyboard() {
    // Toggle signal and immediately notify bridge with the new boolean value
    this.isKeyboardCaptured.update(val => {
      const next = !val;
      // Inform native bridge of new desired state
      this.tauriBridge.toggleListener(next).catch(e => console.error(e));
      return next;
    });
  }

  selectPad(padKey: string) {
    this.selectedPad.set(padKey);
    this.syncMarkerVariables();
    setTimeout(() => {
      this.drawWaveform();
    }, 50);

    if (this.controlBody) {
      this.controlBody.nativeElement.scrollTo({ top: 0, behavior: 'smooth' });
    }
  }

  isPadActive(key: string): boolean {
    return this.playingPads().has(key) || this.fadingPads().has(key);
  }

  hasSound(key: string): boolean {
    return this.audio.hasBuffer(key);
  }

  getDisplayName(path: string | undefined): string {
    if (!path) return 'NO SIGNAL';
    const parts = path.split(/[/\\]/);
    return parts[parts.length - 1];
  }

  getGroupedFiles(files: string[]): [string, string[]][] {
    const groups = new Map<string, string[]>();
    files.forEach(path => {
      const parts = path.split(/[/\\]/);
      const folder = parts.length > 1 ? parts[0] : 'General';
      if (!groups.has(folder)) groups.set(folder, []);
      groups.get(folder)?.push(path);
    });
    return Array.from(groups.entries());
  }

  isExternalFile(path: string | undefined): boolean {
    if (!path) return false;
    return path.startsWith('/') || /^[a-zA-Z]:[\\\/]/.test(path);
  }

  getGainPulsePath(currentVol: number): string {
    const baseline = 11;
    const intensity = (currentVol / 1.5) * 10;
    const peakY = Math.max(1, baseline - intensity);
    return `M 0 ${baseline} C 12 ${baseline}, 12 ${peakY}, 20 ${peakY} S 28 ${baseline}, 40 ${baseline}`;
  }

  togglePadLoop(key: string) {
    this.audio.setLoopState(key, !this.audio.getLoopState(key));
    this.savePadMetadata(key);
  }

  isPadLooping(key: string): boolean {
    return this.audio.getLoopState(key);
  }

  // --- METADATA PERSISTENCE ---
  savePadMetadata(key: string) {
    const trims = this.audio.getTrimParams(key);
    const metadata = {
      name: this.padNames()[key] || '',
      file: this.fileTruth()[key] || '',
      loop: this.audio.getLoopState(key),
      volume: this.audio.getGain(key),
      release: this.audio.getFadeParams(key).release,
      trimIn: trims.in,
      trimOut: trims.out,
      sync: this.padSync()[key] || false,
      bpm: this.padBpm()[key]
    };
    localStorage.setItem(`lsamp_pad_meta_${key}`, JSON.stringify(metadata));
  }

  loadPadMetadata(key: string) {
    const saved = localStorage.getItem(`lsamp_pad_meta_${key}`);
    if (!saved) {
      this.padNames.update(rec => ({ ...rec, [key]: '' }));
      this.fileTruth.update(rec => ({ ...rec, [key]: '' }));
      this.padBpm.update(rec => ({ ...rec, [key]: 120 })); // Initialize default BPM

      if (key === this.selectedPad()) {
        this.syncMarkerVariables();
      }

      return;
    }

    try {
      const meta = JSON.parse(saved);
      this.padNames.update(rec => ({ ...rec, [key]: meta.name || '' }));

      if (meta.bpm) {
        this.padBpm.update(rec => ({ ...rec, [key]: meta.bpm }));
        this.audio.setBpm(key, meta.bpm);
      }

      if (meta.sync !== undefined) {
        this.padSync.update(rec => ({ ...rec, [key]: meta.sync }));
        this.audio.setSyncState(key, meta.sync);
      }

      const rawFile = meta.file || '';
      this.fileTruth.update(rec => ({ ...rec, [key]: rawFile }));

      if (rawFile) {
        let url;
        // Refined absolute path check: Starts with / (Unix) or looks like C:\ (Windows)
        const isAbsolute = rawFile.startsWith('/') || /^[a-zA-Z]:[\\\/]/.test(rawFile);

        if (rawFile.startsWith('music-app:')) {
          url = rawFile;
        } else if (isAbsolute) {
          url = `music-app://system?path=${encodeURIComponent(rawFile)}`;
        } else {
          url = `music-app://harbor/${rawFile}`;
        }

        // Tried to load saved bpm also, but it doesn't work
        this.audio.loadSound(key, url, meta.bpm).then((success: boolean) => {
          if (success) {
            this.loadedPads.update(set => {
              const n = new Set(set);
              n.add(key);
              return n;
            });
          }
        });
      }

      if (meta.trimIn !== undefined && meta.trimOut !== undefined) {
        this.audio.setTrimParams(key, meta.trimIn, meta.trimOut);
      }

      if (meta.loop !== undefined) this.audio.setLoopState(key, meta.loop);
      if (meta.volume !== undefined) this.audio.setGain(key, meta.volume);

      const rel = meta.release !== undefined ? meta.release : (meta.fade ? meta.fade.release : 0.1);
      const att = meta.fade ? meta.fade.attack : 0.1;
      this.audio.setFadeParams(key, att, rel);
    } catch (e) {
      console.error(`[Social Noise] Corruption on Pad ${key}`, e);
    }
  }

  drawWaveform() {
    if (!this.selectedPad() || !this.canvasRef) return;
    const canvas = this.canvasRef.nativeElement;
    const ctx = canvas.getContext('2d');
    const waveform = this.audio.getWaveform(this.selectedPad()!);
    const duration = this.audio.getDuration(this.selectedPad()!);

    if (!ctx) return;

    canvas.width = canvas.offsetWidth;
    canvas.height = canvas.offsetHeight;
    ctx.clearRect(0, 0, canvas.width, canvas.height);

    if (!waveform || waveform.length === 0) return;

    const activeColor = this.accentColor();
    const inactiveColor = 'rgba(255, 255, 255, 0.1)';

    const width = canvas.width;
    const height = canvas.height;
    const amp = height / 2;

    const trim = this.audio.getTrimParams(this.selectedPad()!);
    const startPix = (trim.in / duration) * width;
    const endPix = (trim.out / duration) * width;

    // Draw waveform from downsampled peaks
    const step = width / waveform.length;

    for (let i = 0; i < waveform.length; i++) {
      const peak = waveform[i];
      const x = i * step;
      const h = Math.max(2, peak * height * 0.8);

      const isActive = x >= startPix && x <= endPix;
      ctx.fillStyle = isActive ? activeColor : inactiveColor;

      // Draw centered vertically
      ctx.fillRect(x, (height - h) / 2, Math.max(1, step - 0.5), h);
    }

    // Draw center line
    ctx.fillStyle = 'rgba(255, 255, 255, 0.05)';
    ctx.fillRect(0, height / 2, width, 1);
  }

  // --- NAVIGATION MODULE BRIDGE ---
  handleWheel(event: WheelEvent, type: 'gain' | 'attack' | 'release' | 'trimIn' | 'trimOut' | 'bpm', padKey?: string) {
    const key = padKey || this.selectedPad();
    if (padKey !== 'MASTER' && !key) return;

    event.preventDefault();
    event.stopPropagation();

    const delta = Math.sign(event.deltaY) * -1; // Up is positive, Down is negative
    let step = 0;

    switch (type) {
      case 'bpm':
        step = 0.1;
        if (padKey === 'MASTER') {
          const newMaster = Math.max(1, this.masterBpm() + (delta * step));
          this.updateMasterBpm(parseFloat(newMaster.toFixed(1)));
        }
        // Pad BPM is now read-only (indicative only)
        break;
      case 'gain':
        if (key !== this.selectedPad() || !key) return;
        step = 0.01; // 1%
        const newGain = Math.max(0, Math.min(1.5, this.currentVol() + (delta * step)));
        this.currentVol.set(newGain);
        this.audio.setGain(key!, newGain);
        break;
      case 'attack':
        if (key !== this.selectedPad() || !key) return;
        step = 0.005; // 5ms precision
        const newAttack = Math.max(0, Math.min(4, this.currentAttack() + (delta * step)));
        this.currentAttack.set(newAttack);
        this.audio.setFadeParams(key!, newAttack, this.currentRelease());
        break;
      case 'release':
        if (key !== this.selectedPad() || !key) return;
        step = 0.005; // 5ms precision
        const newRelease = Math.max(0, Math.min(4, this.currentRelease() + (delta * step)));
        this.currentRelease.set(newRelease);
        this.audio.setFadeParams(key!, this.currentAttack(), newRelease);
        break;
      case 'trimIn':
        if (key !== this.selectedPad() || !key) return;
        step = 0.01; // 10ms precision
        this.adjustTrim('in', delta * step);
        break;
      case 'trimOut':
        if (key !== this.selectedPad() || !key) return;
        step = 0.01; // 10ms precision
        this.adjustTrim('out', delta * step);
        break;
    }

    if (key && key !== 'MASTER') {
      this.savePadMetadata(key);
    }
  }

  adjustTrim(type: 'in' | 'out', deltaVal: number) {
    if (this.playingPads().has(this.selectedPad()!)) return;

    this.zone.runOutsideAngular(() => {
      const current = this.audio.getTrimParams(this.selectedPad()!);
      const { attack, release } = this.audio.getFadeParams(this.selectedPad()!);
      const minDuration = Math.max(1, attack + release); // Using 1s minimum safety or combined fade

      let newIn = type === 'in' ? current.in + deltaVal : current.in;
      let newOut = type === 'out' ? current.out + deltaVal : current.out;

      // Clamp to limits
      if (type === 'in') {
        newIn = Math.max(0, Math.min(newIn, newOut - 0.05)); // 50ms minimum slice
      } else {
        newOut = Math.min(this.maxDuration(), Math.max(newOut, newIn + 0.05));
      }

      this.audio.setTrimParams(this.selectedPad()!, newIn, newOut);

      requestAnimationFrame(() => {
        this.drawWaveform();
        const newTrims = this.audio.getTrimParams(this.selectedPad()!);
        this.currentTrimIn.set(newTrims.in);
        this.currentTrimOut.set(newTrims.out);
      });
    });
  }

  updateTrim(event: Event, type: 'in' | 'out') {
    if (!this.selectedPad() || this.playingPads().has(this.selectedPad()!)) return;

    this.zone.runOutsideAngular(() => {
      const target = event.target as HTMLInputElement;
      const val = target.valueAsNumber;
      const current = this.audio.getTrimParams(this.selectedPad()!);

      const { attack, release } = this.audio.getFadeParams(this.selectedPad()!);
      const minDuration = Math.max(1, attack + release);

      let newIn = type === 'in' ? val : current.in;
      let newOut = type === 'out' ? val : current.out;

      if (type === 'in') {
        newIn = Math.min(val, newOut - minDuration);
      } else {
        newOut = Math.max(val, newIn + minDuration);
      }

      this.audio.setTrimParams(this.selectedPad()!, newIn, newOut);

      requestAnimationFrame(() => {
        this.drawWaveform();

        // Manually trigger change detection for UI updates
        const newTrims = this.audio.getTrimParams(this.selectedPad()!);
        this.currentTrimIn.set(newTrims.in);
        this.currentTrimOut.set(newTrims.out);
        this.savePadMetadata(this.selectedPad()!);
      });
    });
  }

  syncMarkerVariables() {
    if (!this.selectedPad()) return;

    const trims = this.audio.getTrimParams(this.selectedPad()!);
    const buffer = this.audio.getBuffer(this.selectedPad()!);

    const duration = (buffer && buffer.duration > 0) ? buffer.duration : 0.001;

    this.maxDuration.set(duration);
    this.currentTrimIn.set(trims.in);
    this.currentTrimOut.set(trims.out);

    this.currentVol.set(this.audio.getGain(this.selectedPad()!));

    const fades = this.audio.getFadeParams(this.selectedPad()!);
    this.currentAttack.set(fades.attack);
    this.currentRelease.set(fades.release);

    requestAnimationFrame(() => this.drawWaveform());
  }

  closeModal() {
    this.showModal.set(false);
    this.modalMode.set(null);
    this.showFinalResetConfirmation.set(false);
  }

  triggerSecondSecurityLayer() {
    this.showFinalResetConfirmation.set(true);
  }

  confirmFactoryReset() {
    localStorage.clear();
    this.closeModal();
    console.log("[Consonance] System Memory Zeroed.");
    window.location.reload();
  }

  executeClearSelected() {
    if (!this.selectedPad()) return;
    const target = this.selectedPad()!;

    this.audio.stopSound(target);
    this.audio.unloadSound(target);

    this.playingPads.update(set => {
      const newSet = new Set(set);
      newSet.delete(target);
      return newSet;
    });

    this.fileTruth.update(rec => {
      const nr = { ...rec };
      delete nr[target];
      return nr;
    });

    this.padNames.update(rec => {
      const nr = { ...rec };
      delete nr[target];
      return nr;
    });

    this.padBpm.update(rec => {
      const nr = { ...rec };
      delete nr[target];
      return nr;
    });

    this.loadedPads.update(set => {
      const n = new Set(set);
      n.delete(target);
      return n;
    });

    this.currentTrimIn.set(0);
    this.currentTrimOut.set(0);
    this.maxDuration.set(0);

    localStorage.removeItem(`lsamp_pad_meta_${target}`);
    this.drawWaveform();
    this.closeModal();
  }

  executeClearAllPads() {
    this.pads().forEach(p => {
      this.audio.stopSound(p);
      this.audio.unloadSound(p);
      localStorage.removeItem(`lsamp_pad_meta_${p}`);
    });

    // Clear loaded set and reload UI
    this.loadedPads.set(new Set());
    window.location.reload();
  }
}
