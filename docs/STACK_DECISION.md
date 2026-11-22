# STACK_DECISION — KineForge

Date: 2026-11-14. Status: **stack and architecture approved 2026-11-22**; the
open decisions in §6 items 2–8 are still pending. Nothing is installed or
implemented yet. Evidence: [TECH_STACK_RESEARCH.md](TECH_STACK_RESEARCH.md),
[WEB_PLATFORM_RESEARCH.md](WEB_PLATFORM_RESEARCH.md),
[OPEN_DIRECTIONAL_DPI_AUDIT.md](OPEN_DIRECTIONAL_DPI_AUDIT.md). Decisive
claims were adversarially re-verified against primary sources
(TECH_STACK_RESEARCH.md §9).

## 1. The recommendation in one table

| Concern | Selected | Main alternatives rejected |
|---|---|---|
| Language / toolchain | Rust stable, pinned exactly (`rust-toolchain.toml`, currently 1.100.0), edition 2024, committed `Cargo.lock`, `--locked` | — |
| Rendering | **Thin custom renderer on wgpu 30** (WGSL shaders, written for the WebGL2 subset) | Bevy, Fyrox, macroquad, SDL3-GPU, raw Vulkan |
| Native backends | DX12 default on Windows (Vulkan selectable), Vulkan on Linux (GL fallback), Metal on macOS | — |
| Web graphics | WebGPU with runtime WebGL2 fallback (`new_instance_with_webgpu_detection`); split into two artifacts only if measured size demands | WebGPU-only (excludes most Linux users) |
| Window / event loop | **winit 0.30.x**, used for windows, surfaces, keyboard and focus — *not* as the research input source; isolated in one module for the 0.31 migration | SDL3 (web only via Emscripten, where wgpu loses WebGPU) |
| Mouse input | **Own `kf-input` layer** with per-platform raw backends and OS timestamps (§4) | winit `DeviceEvent` as the measurement source (no timestamps; accelerated on macOS; high-poll issues on Windows) |
| UI | **egui 0.37 + egui-wgpu + egui_plot**, same code native and web, rendered in the scene's wgpu pass | DOM/TS UI, Slint, iced, Dioxus, Xilem |
| Math | **glam 0.34** — `f64` types in the deterministic core (scalar-only), `f32` SIMD only in the renderer; `libm =0.2.16` for transcendentals | nalgebra, ultraviolet |
| Collision | **Hand-written** ray–sphere/capsule/box tests (exact arithmetic) | parry3d, rapier |
| RNG | **Own xoshiro256++ + samplers** (~40 lines, golden-value tests) | `rand`'s `StdRng`/`SmallRng`/distributions (no reproducibility guarantee) |
| Directional gain | **`kf-dirgain`: `no_std` Rust port of Open Directional DPI**, C engine kept as oracle | C FFI (needs clang for every wasm build) |
| ECS | **None** | Bevy ECS, hecs |
| Raw telemetry | **Custom append-only chunked log `KFLOG`** (fixed-layout LE records, per-chunk CRC-32; std + `crc32fast`) | Arrow IPC, SQLite, NDJSON, postcard |
| Derived data / interchange | **Parquet** (arrow-rs 60; zstd native, pure-Rust codecs on web) + JSON manifest + stored-ZIP export (bundle carries `FORMAT.md` as text; the stdlib Python reader is published separately with its SHA-256) | SQLite (deferred), bincode (unmaintained) |
| Browser storage | OPFS `FileSystemSyncAccessHandle` in a plain JS dedicated Worker; `persist()`; Blob/`showSaveFilePicker` export | IndexedDB as the log store, sqlite-wasm |
| Web build | `wasm32-unknown-unknown` + wasm-bindgen (CLI version taken from `Cargo.lock`) + binaryen `wasm-opt`; single-threaded stable Rust | wasm threads (nightly-only), Emscripten |
| Web hosting | **Cloudflare Pages or Netlify** with COOP/COEP/CSP via `_headers`; optional GitHub Pages mirror with service-worker COI | GitHub Pages as the canonical host |
| CI | GitHub Actions native runners for every OS/arch (`ubuntu-24.04[-arm]`, `windows-2025`, `windows-11-arm`, `macos-15/26`, `macos-15-intel`), wasm compile checks, `wasm-bindgen-test`, one Playwright smoke test | cross-compiling as the primary path |
| Supply chain | cargo-deny, cargo-audit; cargo-auditable for release binaries | — |
| Packaging | cargo-dist for release archives; cargo-packager (or per-OS scripts) for .app/.dmg, MSI, deb/AppImage; signing on native runners | — |
| Profiling / benchmarks | `profiling` facade + Tracy (GUI pinned to the version tracy-client supports); in-app frame/input counters in every session; criterion (wall-time) + gungraun (instruction counts, Linux CI) | — |

## 2. Weighted decision matrices

Scores 1–5 (5 best; for dependency risk 5 = lowest risk). Score inputs are the
evidence tables in TECH_STACK_RESEARCH.md. Weights follow your priority order:
input correctness, portability, performance, maintainability, modularity.

### 2.1 Platform + rendering stack

Weights: input correctness 25 %, portability incl. web 20 %, latency/performance control 15 %, maintainability 15 %, modularity 10 %, dependency risk 10 %, licensing 5 %.

| Stack | Input | Portability | Latency ctl | Maint. | Modularity | Dep. risk | Licence | **Weighted** |
|---|---|---|---|---|---|---|---|---|
| **winit 0.30 + wgpu 30 thin renderer + own input layer** | 4 | 5 | 4 | 4 | 5 | 4 | 5 | **4.35** |
| SDL3 (native) + wgpu + separate web layer | 4 (5 native / 3 web) | 3 | 4 | 3 | 4 | 3 | 5 | 3.60 |
| Bevy 0.20 + own input layer | 3 | 4 | 3 | 2 | 3 | 2 | 5 | 3.05 |
| three-d / kiss3d | 2 | 4 | 3 | 3 | 3 | 3 | 5 | 3.05 |
| ash / vulkano | 3 | 1 | 5 | 2 | 3 | 3 | 5 | 2.85 |
| Fyrox | 2 | 3 | 3 | 3 | 2 | 3 | 5 | 2.75 |
| macroquad / miniquad | 2 | 2 | 3 | 3 | 2 | 4 | 5 | 2.65 |

Why Bevy scores low despite being excellent software: two separate wasm builds for WebGPU and WebGL2, a breaking release every 4–5 months with MSRV tracking the newest stable, pipelined rendering that overlaps frames (can be disabled), and a scheduling model we would work around to control *sample input → simulate → record → present*. The recommendation is **not** a claim that a custom renderer is faster; no engine speed comparison is planned because the decision does not rest on speed. PERFORMANCE_PLAN N3 only checks that the thin renderer meets the 360 Hz budget.

### 2.2 UI

Weights: shared native + web 25 %, high-refresh friendliness 15 %, accessibility 15 %, bundle size 10 %, maintainability 15 %, dependency risk 10 %, licensing 10 %.

| Option | Shared | Hi-refresh | A11y | Bundle | Maint. | Dep. risk | Licence | **Weighted** |
|---|---|---|---|---|---|---|---|---|
| **egui 0.37 (+ egui_plot)** | 5 | 5 | 3 (4 native / 2 web) | 3 | 4 | 4 | 5 | **4.25** |
| DOM/TS on web + egui native | 2 | 4 | 4.5 | 5 | 3 | 4 | 5 | 3.63 |
| Dioxus (DOM web + webview native) | 4 | 3 | 4 | 3 | 3 | 3 | 5 | 3.60 |
| Slint 1.18 | 3 | 4 | 2.5 | 3 | 4 | 4 | 3 | 3.33 |
| Xilem 0.4 | 3 | 4 | 3 | 3 | 2 | 2 | 5 | 3.10 |
| iced 0.14 | 3 | 4 | 1 | 3 | 2 | 2 | 5 | 2.80 |

### 2.3 Raw telemetry format

Weights: crash recovery 20 %, cross-language readability 20 %, hot-path cost & size 20 %, wasm compatibility 10 %, schema evolution 10 %, simplicity/no codegen 10 %, total ordering across event kinds 10 %.

| Format | Crash | X-lang | Hot path | Wasm | Evolution | Simple | Ordering | **Weighted** |
|---|---|---|---|---|---|---|---|---|
| **Custom chunked log (KFLOG)** | 5 | 3 | 5 | 5 | 4 | 5 | 5 | **4.50** |
| NDJSON | 4 | 5 | 2 | 5 | 5 | 5 | 4 | 4.10 |
| SQLite | 5 | 5 | 3 | 2 | 4 | 3 | 4 | 3.90 |
| Arrow IPC stream | 3 | 5 | 4 | 3 | 4 | 4 | 2 | 3.70 |
| Parquet as live log | 1 | 5 | 4 | 4 | 4 | 4 | 3 | 3.50 |

The cross-language weakness of a custom format is mitigated by Parquet tables in every export plus a published ~40-line stdlib Python reader (not shipped inside bundles — DATA_SPECIFICATION.md §12.1). NDJSON is the honest runner-up; it loses on per-event allocation/size in the hot path (and 4× size).

### 2.4 Mouse input source (per platform)

Weights: fidelity / pipeline layer 35 %, timestamp quality 20 %, permission & user friction 20 % (5 = none needed), per-device identity 10 %, implementation cost 15 % (5 = cheapest). Evidence: TECH_STACK_RESEARCH.md §3.

| Platform | Source | Fidelity | Timestamp | Permission | Per-device | Impl. | **Weighted** |
|---|---|---|---|---|---|---|---|
| Windows | **Raw Input, own thread, `GetRawInputBuffer`** | 5 | 3 (QPC at drain) | 5 | 5 | 3 | **4.30** |
| Windows | GameInput | 4 (raw-ness unverified) | 4 (µs, base unverified) | 5 | 5 | 2 (no Rust bindings) | 4.00 |
| Windows | winit `DeviceEvent` | 3 (absolute-as-delta bug #2884) | 1 (none) | 5 | 4 | 5 | 3.40 |
| Linux | **evdev** | 5 | 5 (kernel µs, monotonic) | 2 (`input` group exposes all keyboards) | 5 | 4 | **4.25** |
| Linux | own libinput context | 4.5 | 5 | 2 | 5 | 3 | 3.93 |
| Linux | **X11 `XI_RawMotion`, own connection** | 4 | 2 (server ms) | 5 | 4 | 4 | **3.80** |
| Linux | hidraw (L1) | 5 | 3 (read time) | 2 (udev rule) | 5 | 3 | 3.70 |
| Linux | winit `DeviceEvent` | 3 | 1 | 5 | 2 | 5 | 3.20 |
| Linux | **Wayland relative pointer, own queue** | 3 | 3 (µs; ms on wlroots) | 5 | 1 | 2 (feasibility unverified) | **3.05** |
| macOS | **IOHID report callback (non-seize)** | 5 | 5 | 2 (Input Monitoring) | 5 | 3 | **4.10** |
| macOS | CGEvent unaccelerated field 170 | 3 (units unverified) | 4 | 5 | 1 | 3 | 3.40 |
| macOS | GCMouse | 3 (units unverified) | 2 | 5 | 3 | 4 | 3.35 |
| macOS | winit (NSEvent, accelerated) | 1 | 1 | 5 | 1 | 5 | 2.40 |
| Browser | Pointer Lock + `unadjustedMovement` + `pointerrawupdate`/coalesced | — only option; quality depends on browser × OS (WEB_PLATFORM_RESEARCH.md §2) | | | | | — |

## 3. Reasons for each selection (short)

- **wgpu**: the only GPU layer with first-class DX12, Vulkan, Metal, WebGPU **and** WebGL2; the ecosystem (egui 0.37, Bevy 0.20, Slint 1.18, vello) has converged on wgpu 30 + winit 0.30; present mode and `desired_maximum_frame_latency` control natively; headless rendering for tests.
- **winit (for windows only)**: same ecosystem convergence and a web backend; its *input* gaps are verified (no timestamps #1194; macOS accelerated deltas #4581; X11 `Locked` unsupported) and are exactly why input is owned separately.
- **egui**: one UI codebase for native and web, drawn in the same pass; its per-frame cost at HUD scale is expected to be small but is **unmeasured** (PERFORMANCE_PLAN N1/W5 report the egui share). The web-accessibility gap is real and is the main open decision (§6).
- **glam f64 + libm + own RNG + hand-written hit tests**: designed for bit-identical simulation on x86-64, aarch64 and wasm32 using only IEEE-exact operations and a pinned pure-Rust libm (libm's cross-platform identity is inferred from its source, not documented; the design is verified by EXPERIMENT_PLAN E10); glam's f32 SIMD paths differ by platform (summation order; NEON uses FMA in quaternion ops) so they stay out of the core.
- **KFLOG + Parquet**: crash-safe at chunk granularity, identical code path for native files and OPFS, no codegen, tiny in wasm; Parquet for everyone else.
- **ODD port**: pure Rust everywhere including wasm and `no_std`; exactness enforced by vectors + differential fuzzing against the C oracle.

## 4. Input decision

**Principle:** winit is used for windows, focus and cursor grab only (plus one explicitly labelled Wayland fallback source). Every
measurement stream comes from a KineForge capture backend that records the
exact source, unit, layer (L1–L7) and timestamp `(clock, resolution, meaning)`
plus an always-present receipt time. Nothing is labelled "device counts"
unless it is an HID report (L1).

| Platform | Default (no special permission) | Research mode (opt-in permission) | Ground-truth validator |
|---|---|---|---|
| **Windows** | Raw Input on a dedicated thread: message-only window, `GetRawInputBuffer` drain, QPC per drain (one `t_ns` for every packet of a drain; order within it from the per-source `seq`), absolute packets flagged, mouse-class filter drivers detected; owns the process-wide registration (winit device events set to `Never` first); `RIDEV_NOLEGACY` only while a trial holds the cursor (stops the high-poll legacy `WM_MOUSEMOVE` flood on winit's thread) → `os_raw_counts` | same; GameInput evaluated later for µs timestamps (X-W4) | USB capture (USBPcap/analyser) offline; hardware HID injector |
| **Linux X11** | own Xlib/XCB connection selecting `XI_RawMotion` on the root window → `os_unaccelerated` (predicted to equal HID counts on Xorg+libinput with no rotation — unconfirmed until X-L1; ms server timestamps) | **evdev** with `EVIOCSCLOCKID(CLOCK_MONOTONIC)`, never grabbed → `os_raw_counts` with kernel µs timestamps | **hidraw** (L1) where the node is readable — already the case for the Glorious receiver here |
| **Linux Wayland** | relative-pointer `dx_unaccel` + `utime` on a second event queue of the window's `wl_display` if feasible (X-R6); otherwise the labelled `WinitRelativeFallback` source (winit's relative-pointer motion, receipt-only timestamps, no device identity) → `os_unaccelerated` | evdev (as above) | hidraw |
| **macOS** | **undecided until measured on Mac hardware (X-M1)**: GCMouse vs unaccelerated CGEvent field 170 → `os_unaccelerated` with units marked unverified | IOHID report callback with Input Monitoring, **non-seize** open (hidapi seizes by default) → `device_report` | IOHID itself |
| **Browser** | own `requestPointerLock({unadjustedMovement: true})`; the resolved/rejected result is recorded (no silent fallback); `pointerrawupdate` + `getCoalescedEvents()`; COI for timestamps → `browser_locked_unadjusted` or `browser_locked` | — | none in-browser; native validation via hidraw/Raw Input in parallel |

Why not evdev by default on Linux: membership in the `input` group lets any
process read every keyboard (the reason systemd's `uaccess` excludes mice and
keyboards). Research mode should ask for it explicitly, or use a udev rule
scoped to the specific mouse — your decision (§6).

Security note (no action taken): the existing rule
`/etc/udev/rules.d/99-glorious-series-one-pro.rules` makes `hidraw3` world-
readable, and that interface carries a keyboard collection (report ID 3).
KineForge needs only `hidraw2` (mouse reports) and `hidraw4` (vendor config).

## 5. Known limitations (accepted, documented, tested)

1. wgpu cannot report actual presentation time → input-to-photon is measured with an external photodiode rig; in-app numbers are labelled host-processing only.
2. Browser: display-paced (FIFO/rAF); Safari ≈60 Hz rAF by default in versions documented so far (current Safari 26/27 default unverified); no device counts ever; no unaccelerated input on Linux, timestamps 5–20 µs only with COI.
3. WebGL2 fallback: no compute/storage buffers/GPU timestamps; bigger wasm (naga).
4. egui on web: no screen-reader access (speech-synthesis only); if neither WebGPU nor WebGL2 works, egui cannot draw at all — `index.html` carries a static HTML fallback message.
5. winit 0.31 has been in beta for a year; the 0.30 → 0.31 migration will touch the app shell.
6. wgpu breaks its API about quarterly; renderer code is kept thin to absorb this. An open v30 frame-time regression report (#9937) must be benchmarked against v29 before locking the version.
7. Windows code signing for an individual outside the US/Canada needs an OV/EV certificate (Azure Artifact Signing individual identities are US/Canada-only — secondary source).
8. macOS builds, signing and notarisation require macOS runners; no Mac hardware is available locally for manual testing.

## 6. Decisions requested from you

1. **Approve the stack** in §1 (or name changes).
2. **Web accessibility in v1?** If yes, add a DOM settings/results layer on the web (strategy C) from the start; if no, egui everywhere and DOM later.
3. **Linux input permission model** for native research-grade capture (§4): add your user to `input`/ship a udev rule, or rely on permission-free X11/Wayland paths.
4. **Canonical web host**: Cloudflare Pages or Netlify (GitHub Pages cannot send COOP/COEP).
5. **Name**: keep "KineForge" (medium conflict risk; manual trademark search pending) and reserve crates/npm/PyPI/GitHub org names?
6. **Install the Rust toolchain** (rustup, pinned) on this machine when implementation starts — not done, per your instruction.
7. **Aimlabs**: launch it once to unblock the settings/log inventory? Get legal review before any automated injection/capture rig? Commercial status of KineForge?
8. **Open Directional DPI**: answers to the six maintainer questions (smoothing-window cap, saturation carry vs drop, home of trace vectors, MCU reuse, authoritative calibration analysis, profile v2 plans).

## 7. Proposed version constraints

| Crate / tool | Constraint | Note |
|---|---|---|
| Rust | `channel = "1.100.0"` in `rust-toolchain.toml`; workspace `rust-version = "1.100"` (the pinned toolchain) until an MSRV CI job exists | dependencies need ≥ 1.95 (egui 0.37); a lower declared MSRV must be tested before it is claimed |
| wgpu | `"30.0"` | benchmark vs 29 first (#9937); features `webgpu` (default) + `webgl` on wasm |
| winit | `"0.30.13"` | isolate for 0.31 |
| egui / eframe-free integration: egui, egui-wgpu, egui-winit | `"0.37"` | egui_plot `"0.38"` |
| glam | `"0.34"` | core uses `DVec*`; feature `libm` |
| libm | `"=0.2.16"` | exact pin: algorithm changes would change values |
| arrow / parquet | `"60"`, `default-features = false` | codecs chosen explicitly: native `zstd` + `lz4`; wasm `snap`/`lz4` (pure Rust) |
| serde / serde_json | `"1"`, serde_json feature `float_roundtrip` | exact float parsing for simulation-critical JSON values |
| crc32fast | `"1.5"` | |
| zip | `"9"` | stored members only |
| wasm-bindgen / web-sys / js-sys | `"=0.2.130"` (wasm-bindgen exact) | CLI installed at the same version from `Cargo.lock` |
| windows-sys | current at implementation time | Raw Input FFI |
| evdev (or raw `libc` ioctls) | decided in the native-input spike | the `evdev` crate's `timestamp()` assumes REALTIME and hides `EVIOCSCLOCKID` — read raw `timeval` |
| x11rb / wayland-client | current at implementation time | X11 raw capture; Wayland relative pointer |
| hidapi | `"2.6"` (2.6.7, 2026-08-27), Linux hidraw backend (feature name checked at implementation) | ODD vendor-channel transport (`kf-input` feature `odd-hid`); macOS must call the non-exclusive open option |
| objc2 + framework crates | current at implementation time | macOS input |
| binaryen | release binary pinned in CI (`version_133`) | |
| Tracy GUI | the version matching tracy-client (0.14.1 for tracy-client 0.19) | |

All versions are re-checked when implementation starts; `Cargo.lock` is the source of truth thereafter.

## 8. Dependency risk, portability, maintenance, licensing

| Dependency | Risk | Mitigation |
|---|---|---|
| wgpu (quarterly breaking majors) | medium | thin renderer; upgrade on our schedule; headless render tests |
| winit (0.31 long beta) | medium | used only for windowing; one module |
| egui (monthly releases) | low-medium | UI code kept separate from sim; DOM path available |
| arrow/parquet (heavy, frequent majors) | low | only in export/derived paths; `kf-cli` and web export behind features |
| own input layer (`unsafe` OS FFI) | medium | small, per-platform, tested with programmable HID sources (EXPERIMENT_PLAN E1) |
| own log format | low | spec + reader + validator + golden files; Parquet export |
| ODD port | low | C oracle, vectors, differential fuzzer |

**Licensing.** The selected direct dependencies are MIT and/or Apache-2.0 (winit: Apache-2.0 only; libm: MIT; AccessKit metadata MIT OR Apache-2.0). The Parquet codec dependencies are **BSD-3-Clause** (`zstd`/`zstd-sys`, `snap`) and `brotli` is BSD-3-Clause AND MIT if enabled — all permissive. The `hidapi` crate and the bundled C hidapi library (whose upstream offers a choice of licences including a BSD-style one) are to be confirmed by cargo-deny/cargo-about against the actual tree. No GPL/LGPL, no attribution-badge licences (Slint rejected partly for this), no proprietary runtime components. cargo-deny enforces an allow-list (MIT, Apache-2.0, BSD-3-Clause, Zlib, Unicode-3.0 — finalised against the real tree); cargo-about generates notices. ODD is MIT. KineForge's own licence is your decision (MIT OR Apache-2.0 would match the ecosystem).

**Reproducible builds.** Target level: bit-identical *unsigned* native binaries and web bundles from the same commit on the same CI runner image. Measures: exact toolchain pin; `Cargo.lock` + `--locked`; `--remap-path-prefix` for the workspace and `CARGO_HOME` (Cargo `trim-paths` is still unstable); `SOURCE_DATE_EPOCH` for archive/ZIP timestamps; wasm-bindgen CLI and binaryen pinned (from `Cargo.lock` / `version_133`); no `-C target-cpu=native`. Excluded: signatures and notarisation tickets. Verified by a CI job that builds twice in different directories and compares SHA-256 (ROADMAP M1, M13).

**Portability.** Windows, Linux, macOS (x86-64 + ARM64) and modern desktop browsers are first-class; each is built and tested natively in CI. Browser parity is explicitly *not* identical (WEB_PLATFORM_RESEARCH.md §8).
