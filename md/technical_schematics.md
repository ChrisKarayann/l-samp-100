# L-SAMP 100: Technical Architecture & Schematics

## 1. System Overview
The L-SAMP 100 utilizes a **Hybrid Native Architecture**, leveraging Rust for high-performance systems programming and Angular 20 for a reactive, component-based user interface. The two layers communicate via an asynchronous IPC bridge provided by Tauri 2.0.

### Architecture Diagram
```mermaid
graph TD
    subgraph "Frontend Layer (Angular 20)"
        UI[User Interface / DOM]
        Signals[Angular Signals Store]
        Bridge[TauriBridgeService]
        Canvas[Canvas Visualizer 60fps]
        
        UI <--> Signals
        Signals --> Bridge
        Bridge --> Signals
        Signals --> Canvas
    end

    subgraph "Inter-Process Communication (IPC)"
        JSON[Serialized JSON Payloads]
    end

    subgraph "Backend Layer (Rust)"
        Main[Main Thread / Event Loop]
        Cmd[Command Handlers]
        State[Shared Application State]
        Audio[Audio Engine Thread]
        Input[Global Input Hook (rdev)]
        
        Bridge <--> JSON <--> Cmd
        Cmd --> State
        Input --> Main
        Main --> JSON
        Audio <--> State
    end
```

---

## 2. Core Subsystems

### A. The Audio Engine (Rust)
The audio engine is a dedicated thread managed by `cpal` (Cross-Platform Audio Library). It operates independently of the UI thread to ensure consistent sample delivery.

**Signal Path:**
1.  **Voice Allocation**: Voices are dynamic structs stored in a `Vec<Voice>`.
2.  **Sample Fetching**:
    *   **Interpolation**: Linear interpolation is used for sample-rate conversion (`buffer_sr` -> `device_sr`).
    *   **Mixing**: Voices are summed into a single float buffer (`Vec<f32>`).
    *   **Envelope Shaping**: Per-sample gain multiplication for Attack/Release curves.
3.  **Output**: The summed buffer is written to the OS audio callback.

**Concurrency Model:**
*   **State**: `Arc<Mutex<AudioEngineState>>`
*   **Access**:
    *   **Writer**: The IPC handlers (Main Thread) lock the mutex to push new voices or update parameters.
    *   **Reader**: The Audio Callback (Real-time Thread) locks the mutex to read voice data and write audio frames.
    *   **Contention Strategy**: The mutex is held for extremely short durations (microseconds) to prevent blocking the audio thread.

### B. The Frontend State Machine (Angular)
The UI is "Zoneless," meaning it does not rely on `zone.js` for change detection. This significantly reduces overhead during rapid-fire events.

**Data Flow:**
1.  **Input**: User clicks a pad or presses a key.
2.  **Signal Update**: `activePads` signal is updated.
3.  **Effect Trigger**: An Angular `effect()` triggers the `tauriBridge` call.
4.  **Optimistic UI**: The UI updates immediately (highlighting the pad) while awaiting backend confirmation.
5.  **Reconciliation**: Backend events (`pad-finished`, `levels-update`) flow back to update the UI state.

---

## 3. Data Structures

### Voice Struct (Rust)
```rust
struct Voice {
    key: String,
    buffer: Arc<AudioBuffer>, // Shared reference to sample data
    position: f64,            // Floating-point sample index
    playback_rate: f64,       // Pitch/Speed ratio
    looping: bool,
    gain: f32,
    envelope: EnvelopeState,  // Attack/Release tracking
}
```

### IPC Payload (JSON)
```json
{
  "cmd": "audio_play",
  "args": {
    "key": "Q",
    "volume": 0.8,
    "attack": 0.05,
    "release": 1.2,
    "looping": false
  }
}
```

## 4. Performance Characteristics
*   **Latency**: <10ms (System Dependent).
*   **Memory**: Heavy reliance on `Arc` (Atomic Reference Counting) allows audio buffers to be loaded once and shared across multiple voices without duplication.
*   **CPU**: Audio mixing is SIMD-amenable (though currently scalar implementation). Canvas rendering is decoupled from the main thread via `requestAnimationFrame`.
