# ARCHITECTURE — KineForge

Status: proposal for review, 2026-11-14 (revised after two independent
review rounds). Stack choices are justified in
[STACK_DECISION.md](STACK_DECISION.md); data formats in
[DATA_SPECIFICATION.md](DATA_SPECIFICATION.md). Nothing is implemented.

## 1. Shape of the system

KineForge is a **library-first Cargo workspace**. Applications (trainer,
analyzer, calibrator, CLI) are thin shells over libraries, layered so each
layer only pulls in the dependencies it needs. Arrows point from a crate to
what it depends on:

```
 Layer 3  kf-trainer ─▶ everything below          kf-cli ─▶ layers 0–1, kf-store, kf-input(odd-hid)
          web/ (static shell, OPFS worker, service worker — not a crate)

 Layer 2  kf-render ─▶ kf-core            kf-input ─▶ kf-core, kf-dirgain        kf-store ─▶ kf-telemetry

 Layer 1  kf-sim ─▶ kf-core, kf-dirgain   kf-telemetry ─▶ kf-core
          kf-analytics ─▶ kf-core, kf-telemetry          kf-calib ─▶ kf-analytics, kf-dirgain

 Layer 0  kf-core (no deps beyond optional serde)        kf-dirgain (no_std, zero deps)
```

There are no cycles and no edges between `kf-sim` and `kf-telemetry`: both
share the plain-data record types defined in `kf-core`.

**Crate boundaries follow dependency boundaries, not concepts.** A concept
gets its own crate only when (a) its dependency set differs (GPU, OS APIs,
heavy columnar formats), (b) it must build for a different set of targets
(`no_std`, native-only), or (c) an external project should be able to depend
on it alone. Otherwise it is a module. This keeps the crate count at ≈10.

## 2. Crates

| Crate | Layer | Responsibility | Depends on | Targets |
|---|---|---|---|---|
| `kf-core` | 0 | Units & newtypes; `InputLevel`, `EvidenceClass`, `ClaimStatus`, clock-domain enums; **all plain-data record types** (`InputEvent`, `Motion`, `Stamp`, `Source`, `Gap`, `FrameRecord`, `Drain`, `FrameTiming`, `SimControl`, `SimCamera`, `SimTarget`, `SimShot`, `Annotation`) and the immutable `SceneView` passed to renderers; sensitivity mathematics (yaw/pitch per unit, cm/360, FOV h↔v, Hor+/Vert−, monitor-distance matching, profile conversion with `claim_status`-labelled constants) | `serde` (optional) | all incl. wasm |
| `kf-dirgain` | 0 | Bit-exact port of Open Directional DPI: engine, 28-byte profile, CRC, capabilities, **vendor-HID packet encode/decode** (pure); conformance vectors shared with the C oracle | none | all, `#![no_std]` |
| `kf-sim` | 1 | Fixed-tick clock over simulation time τ; camera from integer unit totals; analytic target trajectories; scenario definitions and task families; own seeded RNG + samplers; ray–sphere/capsule/box hit tests; **`score`** module (bit-exact metrics only: hits, kills, accuracy, time-to-hit/acquire, angular error at shot, on-target ticks; declarative score models); **`experiment`** module (seeded condition/block schedules, ABBA/Latin-square counterbalancing, warm-up flags, block annotations, linked one-session-per-block runs); **replay driver** (frame-by-frame, DATA_SPECIFICATION.md §9.1) | `kf-core`, `kf-dirgain`, `glam` (f64 types, `libm` feature), `libm =0.2.16`, `serde` | all incl. wasm |
| `kf-telemetry` | 1 | KFLOG v1 encoder (pure: records → chunk groups), reader (records/iterators), recovery scan + sidecar, validator, migrations, session/analysis manifests, `.kfz` packing; Parquet export behind feature `parquet` | `kf-core`, `crc32fast`, `serde_json` (`float_roundtrip`); optional `parquet`/`arrow`, `zip` | all incl. wasm |
| `kf-analytics` | 1 | Trajectory and statistical metrics — flick segmentation, endpoint error, overshoot, submovements, velocity-threshold reaction time, Fitts throughput, tracking RMS/lag, smoothness, directional bias, confidence intervals, cross-session comparison. Kernels take **2-D time series with a declared unit** (counts, mm, rad) and never assume a camera or 3-D scene. Outputs are `derived`, never part of official scores | `kf-core`, `kf-telemetry` | all incl. wasm |
| `kf-calib` | 1 | Directional-gain fitting, held-out validation, profile comparison, ODD profile export; enforces the calibration-validity rule by input level | `kf-analytics`, `kf-dirgain` | all incl. wasm |
| `kf-input` | 2 | Capture backends (§3) and capability/contamination snapshots; feature **`odd-hid`**: ODD vendor-channel transport (hidapi/hidraw natively, WebHID on wasm) using `kf-dirgain`'s packet codec | `kf-core`, `kf-dirgain`; per target: `windows-sys`; `libc` (+ optional `evdev`), `x11rb`, `wayland-client`; `objc2-*`; `web-sys`; optional `hidapi` | each backend only on its target |
| `kf-store` | 2 | Durable chunk sinks: native writer + sync threads (exclusive lock, space reserve, backlog cap); browser OPFS via the plain-JS worker and Web Locks; export/import with untrusted-input checks | `kf-telemetry`; per target: std fs, `web-sys` | per target |
| `kf-render` | 2 | wgpu presentation of `SceneView`: scene, crosshair, overlays, quality tiers, frame timing hooks | `kf-core`, `wgpu`, `glam` (f32) | native + wasm |
| `kf-trainer` | 3 | Window/event loop (winit), egui UI, app state machine, frame loop, recorder thread; `main.rs` (native) and `lib.rs` (wasm entry) | all of the above, `egui`, `egui-wgpu`, `egui-winit` | native + wasm |
| `kf-cli` | 3 | Headless: validate, inspect, recover, export (Parquet/CSV), replay, re-score, analyse, calibrate, sensitivity conversion, imports (MouseTester CSV, ODD evdev CSV, KovaaK's Stats.csv), ODD profile upload | layers 0–1, `kf-store`, `kf-input` (`odd-hid`) | native |

`web/`: `index.html` (with a static no-GPU fallback message), the
wasm-bindgen output, a ≈30-line OPFS writer worker, the service worker and
`_headers`.

### 2.1 Target-gating and purity rules

- Layers 0–1 contain no `cfg(target_os)` code and no OS/browser dependencies. Because `cargo check --target wasm32-unknown-unknown` does **not** reject `std::fs`/`std::net`/`std::process`/`std::thread::spawn`/`std::time::Instant::now` (they compile and fail at run time), purity is enforced with clippy `disallowed_types`/`disallowed_methods` in layer 0–1 crates — the same mechanism that bans `f64::sin`, `algebraic_*` and `mul_add_relaxed` in deterministic code. CI also runs `cargo check --target wasm32-unknown-unknown` for every layer 0–1 crate and `--target thumbv7em-none-eabihf` for `kf-dirgain`.
- Platform code lives only in layer 2, selected by **target-specific dependency tables**, not user-facing features.
- Cargo features are reserved for optional capability: `serde`, `parquet`, `profiling`, `hid` (device-report capture), `odd-hid`, `c-oracle` (dev-only differential fuzzer).

## 3. Input and recording pipeline

```
 capture thread (per backend) ─try_push─▶ per-source ring (pre-allocated, ≥1 s at 8 kHz)
   │ drop state in atomics (first_dropped_seq, count, t_first)          │
   │ declarations/device events via a never-dropped side channel        ▼
   │                                                           recorder thread ──▶ chunk groups ──▶ writer thread ──▶ file
   │                                                             │      ▲                              (sync thread: fdatasync)
   │                                       try_push (never blocks)│      │ one message per frame (never-block ring)
   │                                                             ▼      │
   │                                                     sim ring ──▶ main loop: controls → drain → step → controls
 web: DOM callback (main thread) → wasm buffers → frame loop → recorder (same thread) → postMessage → OPFS worker
```

**Event types** (`kf-core`, plain data):

```rust
pub struct Stamp { pub ns: i64, pub clock: Clock, pub res_ns: u32, pub meaning: Meaning }
pub struct Motion {
    pub seq: u32,              // one counter per source shared by motion/button/wheel; motion before button within a report
    pub source: u16,           // declared source id (new id per pointer-lock acquisition; absolute packets → own source)
    pub dx: i32, pub dy: i32,  // × the source's value_scale
    pub t_src: Option<Stamp>,  // the source's own stamp, unmodified
    pub t_rx: Stamp,           // receipt on the session clock (always present)
    pub flags: u16,            // COALESCED | BACKGROUND | ABSOLUTE | SUSPECT_INJECTED | CLAMPED | HI_RES
}
pub enum InputEvent {
    Motion(Motion), Button { .. }, Wheel { .. },
    Gap { source: u16, first_seq: u32, lost: u32, reason: GapReason, t_rx: Stamp },
    Declare(SourceInfo), Device(DeviceInfo),
}
```

**Native capture backends** implement one trait (several real
implementations). Stopping uses a backend-provided wake, because backends
block in OS waits and would never observe a flag while the mouse is idle:

```rust
pub trait Capture: Send {
    fn info(&self) -> CaptureInfo;   // source, clock, focus requirement, permission status and why
    fn start(self: Box<Self>, out: CaptureSink) -> Result<Box<dyn StopHandle>, CaptureError>;
}
pub trait StopHandle: Send { fn stop(self: Box<Self>); }  // WM_QUIT/SetEvent, eventfd, X ClientMessage, CFRunLoopStop
```

**Nothing on the capture path ever blocks:**

1. **Capture → recorder.** `try_push` into a pre-allocated per-source ring (≥ 1 s at 8 kHz). On a full ring the capture thread updates per-source drop state in atomics (first dropped seq, count, time of first loss); the recorder turns it into a `queue_overflow` gap annotation at every group flush and at stop — no ring slot needed, so tail losses are never invisible. Declarations, device events and gaps use a small side channel that is never dropped.
2. **Recorder → simulation.** The recorder forwards the driving source's events into a pre-allocated sim ring with `try_push`. On overflow it keeps recording, drops only from the sim path, and records a `sim_queue_overflow` annotation (source, first seq, count); the simulation aborts the running trial (`sim_control abort_trial`, reason `sim_queue_overflow`). Replay skips exactly those seqs.
3. **Main loop → recorder.** Each frame the main loop sends **one message** with all records it produced (frame, drain, sim_*, sim_control, annotations, resolved frame timings) through a never-block ring; on overflow the frame's records are counted in a `record_queue_overflow` annotation. The recorder cuts chunk groups only between frame messages, so a commit never claims a tick whose records are elsewhere.
4. **Recorder → writer.** Groups go to a writer thread through a queue bounded only by a memory cap (default 64 MiB; beyond it groups are dropped and `storage_error` recorded). `fdatasync` runs on a separate sync thread, so a slow sync never stalls writing or recording.

OS-side loss before KineForge numbers events (Windows Raw Input, browsers) is
invisible to `seq` and is detected only by Σcounts against an L1 source.

**Thread priorities** (recorded in source declarations): Windows
`THREAD_PRIORITY_TIME_CRITICAL` for the Raw Input thread; Linux `SCHED_FIFO`
via rtkit if granted, otherwise a raised nice level; macOS
`USER_INTERACTIVE` QoS for the IOHID thread. **Main-thread exceptions:**
macOS NSEvent monitoring and all browser capture (on the web the frame loop
itself feeds the recorder logic; there are no threads).

**Backends** (selection per platform in STACK_DECISION.md §4):

| Backend | Level | Thread | Notes |
|---|---|---|---|
| `RawInputCapture` (Windows) | `os_raw_counts` | dedicated, message-only window, `GetRawInputBuffer` drain, QPC per drain | owns the process-wide mouse registration (winit device events set to `Never` first; last registration wins); `RIDEV_NOLEGACY` **only while a trial holds the cursor** (also stops the high-poll legacy `WM_MOUSEMOVE` flood on winit's thread), re-registered without it for menus; `MOUSE_MOVE_ABSOLUTE` packets routed to a separate `os_absolute_pointer` source; `hDevice == 0`, pen signature and mouse-class `UpperFilters` flagged |
| `EvdevCapture` (Linux) | `os_raw_counts` | dedicated, `poll()` | permission-gated; `EVIOCSCLOCKID(CLOCK_MONOTONIC)`; never `EVIOCGRAB`; `SYN_DROPPED` → gap with `lost = u32::MAX` |
| `HidrawCapture` (Linux) | `device_report` | dedicated | ground truth; descriptor parsed from sysfs; stamp at `read()` |
| `X11RawCapture` | `os_unaccelerated` | own `Display` connection | `XI_RawMotion` on root for all devices (`sourceid`), permission-free, global; Xwayland detected and labelled |
| `WaylandRelativeCapture` | `os_unaccelerated` | second event queue on the window's `wl_display` | keeps `utime`; feasibility to be shown (X-R6) |
| `WinitRelativeFallback` | `os_unaccelerated` | main thread | only if the Wayland queue is infeasible; receipt-only timestamps, no device identity — labelled so |
| `IoHidCapture` (macOS) | `device_report` | own run loop / dispatch queue | Input Monitoring; **non-seize** open |
| `GcMouseCapture` (macOS) | `os_unaccelerated` (units unverified) | GameController handler queue | foreground only |
| `WebCapture` | `browser_locked[_unadjusted]` | main thread | own `requestPointerLock({unadjustedMovement:true})`; result recorded per source; exactly one motion event type; leaf coalesced events only; a new source per lock acquisition (re-locks that change level/units end the session) |

## 4. Simulation

- **Simulation time τ** advances only while running (pauses excluded); ticks (default 1000 Hz) are steps of τ; spawns, lifetimes and scoring windows are functions of τ. Hit tests evaluate targets at τ(click time) clamped to the target's lifetime.
- **Frame procedure** (identical live and in replay): apply `sim_control` with phase *before drain* → apply the driving source's events one by one in `seq` order (discarded while paused) → step the frame's ticks → apply `sim_control` with phase *after drain*. `frame`, `drain` and `sim_control` records make this exactly reproducible (DATA_SPECIFICATION.md §9.1).
- **One driving source at a time**; switches only between pause and resume and only to a source with identical level/units/scale; otherwise the session ends.
- **Camera from integer totals**: `yaw_units`/`pitch_units` (`i64`); orientation = units × (rad_per_unit × sens) / value_scale, recomputed every tick. Sensitivity, FOV, gain profile and `invert_y` are immutable per session.
- **Shots** use the camera state immediately after the last motion record preceding the click in `seq` — not the per-tick state — so batching never changes aim.
- **Directional gain** only on `counts`/`value_scale = 1` sources; per event, `seq` order, ODD's capped-carry policy recorded. **Pitch clamp** in unit space per event.
- **Every non-input state change** (start/restart trial from keyboard or UI, pause/resume, abort, next block, source switch, end) is a `sim_control` record.
- **Determinism**: own xoshiro256++ + samplers with golden vectors; pinned `libm`; f64 glam types only; no fast-math; no `HashMap` iteration in sim logic; per-tick state hash in test builds. Cross-target bit-identity is a design goal **verified by EXPERIMENT_PLAN E10**, not assumed.
- **No ECS**: a few `Vec`s of targets plus a camera.

## 5. Rendering and frame loop

```
controls(before) → take driving-source input up to cutoff (late as possible) → step sim → controls(after)
  → send frame message to recorder → build SceneView → encode + submit → present → (timings resolved later → frame_timing)
```

`kf-render` reads only `SceneView` (in `kf-core`); it never feeds back into
simulation state, and headless tools do not link it.

**Pipeline (wgpu, WGSL, written for the WebGL2 subset — no compute, no storage buffers):**

1. *Scene pass*: environment geometry (static vertex buffers) + targets instanced from a per-frame instance buffer; depth buffer; MSAA by quality tier.
2. *Overlay pass*: crosshair (screen-space, pixel-snapped; the snap offset is recorded in `render_state` records) and debug lines.
3. *UI pass*: egui via egui-wgpu into the same surface texture.
4. Present. Native: user-selectable `AutoNoVsync`/`Mailbox`/`Fifo`, `desired_maximum_frame_latency = 1` by default, DX12 default on Windows. Web: FIFO/rAF only.

**Frame cap** (native, uncapped modes): optional `frame_cap_hz` via a
high-resolution timed wait plus a short spin before input sampling (Windows
high-resolution waitable timers to be verified); recorded in the header's
`render` block and `render_state` records. All pipelines are created at load
time. Quality tiers change resolution scale, MSAA and environment detail
only. Surface loss (native) → reconfigure; device/context loss (web) → abort
trial, rebuild the renderer from `SceneView`, continue. EXPERIMENT_PLAN E11
checks that the f32 renderer draws targets where the f64 simulation says they
are, on every backend.

## 6. Telemetry and storage

- `kf-telemetry` encodes groups (declarations first, kinds ascending, `commit` last) of the single append-only `raw.kflog` per session (DATA_SPECIFICATION.md §5) — pure Rust, no I/O.
- **Native**: recorder → writer thread (`write` per group) + sync thread (`fdatasync` every 1000 ms); exclusive file lock; ≥ 64 KiB space reserve so a clean `storage_error` end can always be written; close = final group → commit → `session_end`.
- **Web**: the frame loop runs the recorder logic and posts encoded chunk bytes (transferable `ArrayBuffer`) to a ≈30-line plain-JS dedicated Worker owning the OPFS `FileSystemSyncAccessHandle`; the tab holds a Web Lock on the session id; a small OPFS reserve file is released on `QuotaExceededError`; on `visibilitychange: hidden`/`pagehide` a complete group with its commit is posted and flushed. Without a sync access handle the app does not record (practice only) — there is no IndexedDB log store.
- Recovery never modifies `raw.kflog`; it writes a sidecar and defines the recovered session's identity hash (DATA_SPECIFICATION.md §5.4).
- Derived Parquet tables are produced at session close or by recovery; cross-session artifacts (calibrations, studies) live under `analyses/`.
- Export: stored-ZIP `.kfz` (raw log, manifest, Parquet tables, `FORMAT.md` as text); the reference reader is published separately with its hash.

## 7. External API boundaries

| Boundary | Form |
|---|---|
| Rust consumers | the crates (semver per crate once published) |
| Other languages | data, not code: session bundles read with Parquet/JSON tooling or the published stdlib Python reader. A C ABI only if a non-Rust consumer needs live calls |
| Firmware | ODD profile bytes and vendor-HID packets via `kf-dirgain` (same bytes as the C implementation); transport via `kf-input` feature `odd-hid` (hidapi/hidraw natively, WebHID on Chromium) |
| Browser JS | minimal `wasm-bindgen` surface on `kf-trainer` (start, resize, capability report, export/import) + the OPFS worker message protocol (chunk bytes in, flush/ack out) |

## 8. Future applications (Phase 5) and what they link

| # | Application | Links | Does not link | Targets |
|---|---|---|---|---|
| 1 | Aim trainer (native + browser) | everything | — | native + wasm |
| 2 | Mouse sensitivity analyzer | `kf-core`, `kf-input`, `kf-telemetry`, `kf-analytics` | `kf-sim`, `kf-render` (2-D plots via egui) | native (full), wasm (browser-level input) |
| 3 | ODD calibration application | `kf-core`, `kf-input` (`odd-hid`), `kf-telemetry`, `kf-analytics`, `kf-calib`, `kf-dirgain`; plus `kf-sim`/`kf-render` when it runs its own centre-out task | — | native; wasm for indicative calibration + WebHID upload (Chromium) |
| 4 | Input latency research tool | `kf-core`, `kf-input`, `kf-render`, rig host code | `kf-sim`, `kf-analytics` | native |
| 5 | Flick trajectory analyzer | `kf-telemetry`, `kf-analytics` | `kf-input`, `kf-sim`, `kf-render` | native + wasm |
| 6 | Tracking performance analyzer | `kf-telemetry`, `kf-analytics` | `kf-input`, `kf-sim`, `kf-render` | native + wasm |
| 7 | Sensitivity conversion toolkit | `kf-core` only | everything else | CLI + wasm page |
| 8 | Scientific experimentation environment | `kf-sim` (`experiment`), trainer shell, `kf-analytics` | — | native + wasm |
| 9 | Firmware calibration/validation tools | `kf-dirgain`, `kf-calib`, `kf-input` (`odd-hid`) | `kf-render`, `kf-sim` | native |
| 10 | Data visualisation app | `kf-telemetry`, `kf-analytics` + egui_plot, or Parquet + external tools | `kf-input`, `kf-sim` | native + wasm |
| 11 | Benchmark and replay tool | `kf-telemetry`, `kf-sim` (replay), optional `kf-render` | `kf-input` | native + wasm |
| 12 | Future input-processing algorithms | `kf-core` + counterfactual replay in `kf-sim` | — | native + wasm |
| 13 | Browser analytics & local-first calibration | as 5/6 + `kf-store` (OPFS) | native backends | wasm |
| 14 | Educational visualisations / shareable benchmarks | `kf-core`, `kf-render` or egui; privacy export | `kf-input` | wasm |
| — | Python/R analysis | session bundles only | all Rust | any |

## 9. Cross-platform abstractions (complete list)

1. Input capture (§3) — the `Capture` trait natively, DOM callbacks on the web, one shared event type.
2. Durable chunk sink in `kf-store` — native writer/sync threads vs OPFS worker.
3. The window/surface + event loop — provided by winit, not wrapped by KineForge.

Everything else is plain portable Rust.
