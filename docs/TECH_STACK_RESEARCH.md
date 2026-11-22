# TECH_STACK_RESEARCH — KineForge

Research dates 2026-11-13/14. Versions, dates and licences come from the
crates.io API (`https://crates.io/api/v1/crates/<name>`), GitHub (`gh api`),
and source code at the cited tags; every source was accessed on those dates.
**UNVERIFIED** marks anything not confirmed from a primary source. Decisive
claims were re-checked by an independent adversarial verification pass;
corrections from that pass are folded in and listed in §9.

Working notes with full source lists: [`research/notes/`](../research/notes/)
(`rust_render_ui.md`, `rust_math_data.md`, `input.md`, `web_platform.md`,
`_windows.md`, `_browser.md`, `_rust.md`).

Local environment (verified 2026-11-13): Linux Mint 22.3, kernel 7.0, **X11**
Cinnamon session; Ryzen 7 5700G; NVIDIA RTX 3070 Ti (driver 580.178.04,
Vulkan 1.4) + AMD Vega iGPU (RADV); 2560×1440 @ 165 Hz; Chromium 156,
Firefox 159, Node 24; gcc, python3; **no Rust toolchain, no clang, no
cmake**; user not in the `input` group but has an ACL on `/dev/uinput`.

---

## 1. The layers are different things

| Layer | Candidates | Note |
|---|---|---|
| GPU abstraction | wgpu, ash, vulkano, blade-graphics, glow | wgpu is the only one spanning DX12/Vulkan/Metal/GL **and** WebGPU/WebGL2 |
| Window + event loop | winit, SDL3 (via `sdl3` crate), miniquad | provides surfaces and OS events; *not* a trustworthy research-grade mouse source on its own (§3) |
| Engines | Bevy, Fyrox, macroquad | bundle the above plus scheduling, assets, ECS |
| UI | egui, iced, Slint, Dioxus, Xilem, DOM | §4 |
| Input capture | winit `DeviceEvent`, SDL3, direct OS APIs, browser Pointer Lock | §3 |

## 2. Rendering and windowing

### 2.1 Candidates (crates.io / GitHub, 2026-11-13)

| Crate | Latest stable (date) | Licence | MSRV | Native backends | Browser | Maintenance | Limitations for KineForge |
|---|---|---|---|---|---|---|---|
| **wgpu** | 30.0.2 (2026-11-12) | MIT OR Apache-2.0 | 1.87 | DX12, Vulkan, Metal, GL 3.3/GLES 3.0 | WebGPU (first-class), WebGL2 (downlevel, naga WGSL→GLSL) | very active; breaking major ≈ every 3 months | no actual-present-time API (#2869, #9856); web = FIFO only; open v30 frame-time regression report #9937; WebGL2 has no timestamp queries |
| **winit** | 0.30.13 (2026-03-02); 0.31.0-beta.4 (2026-10-24) | Apache-2.0 | 1.70 (0.31: 1.86) | Win32, AppKit, X11, Wayland | canvas + DOM events | active; 0.31 in beta for 12 months | no event timestamps (#1194); macOS raw motion is accelerated (#4581); Windows high-poll-rate fix still an open PR (#3609); `Locked` grab unsupported on X11, `Confined` unsupported on macOS/web |
| Bevy | 0.20.1 (2026-10-28) | MIT OR Apache-2.0 | 1.97.1 | via wgpu 30 + winit 0.30 | WebGPU **or** WebGL2 — separate builds (the `webgpu` feature overrides `webgl2`) | very active; breaking release ≈ every 4–5 months | heavy tree (bevy_internal: 15 required + 43 optional direct deps); pipelined rendering overlaps sim N+1 with render N; inherits winit's input gaps; wasm size unpublished (UNVERIFIED) |
| sdl3 / sdl3-sys | 0.22.1 (2026-11-09) / 0.7.3+SDL 3.4.20 | MIT / Zlib | — / 1.85 | SDL3 platforms | **Emscripten only**; wgpu compiles out WebGPU on emscripten → GLES only | README: "Expect some bugs and missing features" | needs a second platform layer for `wasm32-unknown-unknown`; C in the build |
| macroquad / miniquad | 0.4.16 / 0.4.11 | MIT OR Apache-2.0 | — / 1.70 | GL2/3, GLES, Metal | WebGL 1 with its own JS loader (not wasm-bindgen) | slower | no WebGPU/DX12/Vulkan |
| Fyrox | 1.0.1 (2026-03-28) | MIT | 1.87 | GL (glow) | WebGL2 | active repo | editor-centric engine, GL only |
| ash / vulkano | 0.38.0 (2024-04-01) / 0.35.2 | MIT OR Apache-2.0 | 1.69 / 1.75 | Vulkan only | none | ash: no release in 2.5 years | no web, no DX12/Metal |
| blade-graphics | 0.9.0 (2026-09-06) | MIT | 1.92 | Vulkan, Metal, basic GLES | basic WebGL2, no WebGPU | active | no DX12 |
| three-d / kiss3d | 0.19.0 / 0.47.0 | MIT / BSD-3 | — | GL / wgpu | WebGL2 / via wgpu | small | viewer-oriented APIs |
| rend3 | 0.3.0 (2022) | — | — | — | — | **archived** | excluded |

### 2.2 wgpu details that matter (verified in v30.0.2 source)

- **Present modes**: `Fifo` everywhere (default); `Immediate` on most platforms except older DX12 and Wayland; `Mailbox` on DX12 (Win10+), NVIDIA Vulkan, Wayland Vulkan; `FifoRelaxed` AMD Vulkan. **Web backend reports `[Fifo]` only and panics on other modes.** GLES reports Fifo + Immediate on Windows, Fifo only elsewhere.
- **`desired_maximum_frame_latency`** (default 2; 1 = lowest latency, CPU and GPU no longer overlap): Vulkan → swapchain images N+1; DX12 → `SetMaximumFrameLatency(N)`; Metal → `maximumDrawableCount` N+1; **OpenGL: ignored**.
- **Runtime WebGPU→WebGL2 fallback in one binary**: `wgpu::util::new_instance_with_webgpu_detection` (enable both `webgpu` and `webgl`). Cost: `webgl` puts naga into the wasm; egui measured ≈ +2.4 MiB (5.9 → 8.3 MiB uncompressed demo) for wgpu+WebGL vs glow (egui #5889). Alternative: two artifacts chosen by a small JS loader.
- **Timestamp queries**: native backends yes; WebGPU only when the browser exposes `timestamp-query`, and not inside encoders; WebGL2 none.
- **No presentation feedback**: wgpu cannot report when a frame actually reached the display. Input-to-photon must be measured externally (photodiode) or via backend-specific `as_hal` escapes (DXGI frame statistics, `VK_KHR_present_wait`, `CAMetalDrawable.presentedTime`) — feasibility UNVERIFIED.
- **Headless**: Instance/Adapter/Device without a Surface, rendering to textures — standard and suitable for deterministic render tests.
- **Shaders**: WGSL is the single source language; naga translates for native and WebGL2; passes through to browsers' WebGPU.
- **Debugging**: `Device::start_graphics_debugger_capture` drives RenderDoc (v1.47) and Xcode GPU capture; PIX attaches to DX12.

### 2.3 Thin renderer vs engine; one renderer for all targets?

An aim trainer needs a handful of pipelines (instanced targets, a simple
environment, crosshair, UI, debug lines). A thin wgpu renderer (estimated a
few hundred to low thousands of lines — an estimate, not a measurement) gives
direct control of present mode, frame latency and the frame loop *sample input
→ simulate → record → encode → present*, which is what this project is about.
An engine would add scheduling we would have to fight (Bevy's pipelined
rendering, ECS stages), a breaking-change cadence of 4–5 months, and on the
web two separate wasm builds for WebGPU and WebGL2.

**This is a control and correctness argument, not a speed claim.** No
benchmark here shows a custom renderer is faster than Bevy, and no engine
speed comparison is planned; PERFORMANCE_PLAN N3 only checks that the thin
renderer meets the 360 Hz budget. The decision rests on: latency
control, frame-loop ordering, dependency churn, wasm size, and keeping the
simulation independent of any engine.

One wgpu renderer with WGSL shaders serves all targets. Web-specific
differences are confined to: FIFO/rAF pacing, the WebGL2 downlevel subset
(no compute, no storage buffers in the WebGL2 path), and no GPU timestamps on
WebGL2. A separate presentation adapter per target (strategy B in
WEB_PLATFORM_RESEARCH.md) would duplicate the renderer with no evident gain.

### 2.4 Score inputs (1–5, higher is better)

| Stack | Input correctness | Portability incl. web | Latency control | Maintainability | Modularity | Dependency risk (5 = low) | Licensing |
|---|---|---|---|---|---|---|---|
| **winit 0.30 + wgpu 30 thin renderer + own input layer** | 4 (2 without the input layer) | 5 | 4 | 4 | 5 | 4 | 5 |
| SDL3 native + wgpu + separate web layer | 5 native / 3 web | 3 | 4 | 3 | 4 | 3 | 5 |
| Bevy 0.20 | 2 | 4 | 3 | 2 | 3 | 2 | 5 |
| macroquad/miniquad | 2 | 2 | 3 | 3 | 2 | 4 | 5 |
| Fyrox | 2 | 3 | 3 | 3 | 2 | 3 | 5 |
| three-d / kiss3d | 2 | 4 | 3 | 3 | 3 | 3 | 5 |
| ash / vulkano | 3 | 1 | 5 | 2 | 3 | 3 | 5 |
| blade-graphics | 2 | 2 | 4 | 3 | 3 | 3 | 5 |

## 3. Input technologies (summary — full detail with source lines in `research/notes/input.md`)

**Cross-cutting facts.** USB HID mouse reports carry **no timestamp**; every
timestamp is a host-side instant with a platform-specific meaning. **True
device counts** (HID report X/Y) are readable only through Linux hidraw and
macOS IOHID; Windows opens mouse collections exclusively and browsers protect
them. Even HID counts already include firmware processing (CPI scaling, angle
snapping, smoothing, lift-off, motion sync, wireless batching).

### 3.1 Native capture paths

**Windows**
- **Raw Input** (`WM_INPUT`/`GetRawInputBuffer`): relative `RAWMOUSE.lLastX/Y` with **no ballistics** (bypasses Enhanced Pointer Precision); per-device `hDevice`; **no timestamp field** — the app stamps QPC at dequeue. `MOUSE_MOVE_ABSOLUTE` packets (RDP, VMs, tablets) are normalised 0–65535 coordinates, not deltas (`MOUSE_MOVE_RELATIVE == 0`, so `flags & RELATIVE` tests are always true — winit has exactly this bug, #2884). Kernel filter drivers can rewrite "raw" input (RawAccel hooks mouclass) → check mouse-class `UpperFilters`.
- **One registrant per device class per process** (last call wins); winit also registers mice → set winit device events to `Never` before registering.
- **High polling rates**: drain with `GetRawInputBuffer` on a dedicated thread (message-only window, `MsgWaitForMultipleObjects(QS_RAWINPUT)`); Godot measured an 8 kHz mouse dropping a game below 1 FPS without this (2026-08). Windows 11 throttles/coalesces raw input for **background** listeners (Microsoft blog 2023-05-26; the "125 Hz" figure is unverified).
- **GameInput** (Microsoft.GameInput 4.0.306): per-reading µs timestamps and per-device integer deltas — the only Windows mouse API with a per-reading timestamp; base and per-report granularity unverified; no maintained Rust bindings; raw HID reports only for GIP devices.
- Cursor APIs (`WM_MOUSEMOVE`, `WM_POINTER`, `GetCursorPos`) are post-ballistics and pixel-quantised.

**Linux**
- **evdev**: HID logical values unchanged, `input_sync` once per HID report; **zero-valued axes dropped, all-zero reports invisible**; no acceleration in the kernel; timestamp = host time at the input core's flush of the report's `SYN_REPORT`, µs, **CLOCK_REALTIME by default** (`EVIOCSCLOCKID` → MONOTONIC); per-client ring of 128 events here (≈5 ms at 8 kHz → `SYN_DROPPED`); `EVIOCGRAB` would starve the desktop — never grab. Nodes are `root:input 0660`; systemd's `uaccess` deliberately excludes mice/keyboards.
- **hidraw**: unmodified report bytes; reading does not take the mouse from the OS (the kernel feeds hidraw and hid-input from the same report); HID-BPF programs can rewrite reports before both. No timestamp (stamp at `read()`).
- **libinput**: `dx_unaccelerated` = device units + configured rotation (**not** DPI-normalised — only accelerated motion is normalised); timestamps µs on CLOCK_MONOTONIC (kernel flush time); flat profile at speed 0 ⇒ accelerated == unaccelerated; Lua plugins (≥ 1.30) can alter events.
- **X11 (Xorg + xf86-input-libinput)**: `XI_RawMotion` raw values = libinput unaccelerated; **global and permission-free** for any X client (XI ≥ 2.1), per physical device via `sourceid`; timestamps are X server dispatch time in ms (`CLOCK_MONOTONIC_COARSE`). Under Xwayland, raw = `dx_unaccel × surface scale` and only while focused.
- **Wayland**: `zwp_relative_pointer_v1` gives `dx_unaccel` (wl_fixed 1/256) and `utime` (µs, base unspecified; **wlroots/sway send ms × 1000**); the protocol says unaccelerated motion "does not represent 'raw' events" — Mutter, KWin and wlroots pass libinput's values; focus-scoped only (no background capture); `zwp_pointer_constraints_v1` for lock/confine. winit's Wayland backend drops `utime` and device identity.

**macOS**
- `NSEvent.deltaX/Y`: accelerated, coalesced by default (what winit and SDL 3.4 use).
- `kCGEventUnacceleratedPointerMovementX/Y` (public field 170/171): pre-acceleration (IOHIDFamily keeps the original event and appends an accelerated child); units relative to HID counts **unverified**.
- **GCMouse** (macOS 11+): Apple calls its deltas "raw"; float, no per-event timestamp, foreground-only by default; units unverified. SDL `main` adopted it (SDL PR #14741, 2026-03).
- **IOHID** report callbacks: true HID reports with AbsoluteTime timestamps; **Input Monitoring (TCC)** required for any mouse/keyboard/touchpad device; must open **non-seize** (hidapi seizes by default on macOS and would steal the mouse).

### 3.2 Cross-platform Rust libraries

| Crate | Version (date) | What it gives | Verdict |
|---|---|---|---|
| winit | 0.30.13 | `DeviceEvent::MouseMotion`: Windows Raw Input (absolute-as-delta bug), X11 XI2 raw, Wayland unaccel (no `utime`), macOS **accelerated** NSEvent, web movementX; **no timestamps** | windowing only |
| sdl3 / sdl3-sys | 0.22.1 / 0.7.3+SDL 3.4.20 | relative mode; ns timestamps — but Windows raw timestamps are **interpolated** across each drain, X11 relative timestamps leak X-server ms into the ns field (source-verified), macOS 3.4 deltas OS-processed | not a measurement source; useful reference design (Windows raw thread) |
| evdev | 0.13.2 (2025-09-15) | `/dev/input/event*`, uinput | usable; its `timestamp()` assumes REALTIME and `EVIOCSCLOCKID` is private — set the clock via the raw fd and read raw `timeval` |
| input (libinput) | 0.10.0 | unaccelerated + accelerated + µs time | alternative if libinput semantics are wanted |
| hidapi | 2.6.7 | HID reports / feature reports | vendor channels (ODD config); mouse reports only on Linux/macOS (non-seize) |
| rusb / nusb | 0.9.4 / 0.2.9 | USB transfers | ✘ for live capture (claiming the interface detaches the OS driver) |
| multiinput | 0.1.0 (2020) | Windows Raw Input | stale — write own with `windows-sys` |
| device_query, rdev | 4.0.1, 0.5.3 | polled cursor / global hooks | ✘ — accelerated cursor pixels, wall-clock times |
| gilrs | 0.11.2 | gamepads | irrelevant |

### 3.3 Browser input

Summarised in WEB_PLATFORM_RESEARCH.md §2: OS-raw deltas only with
`unadjustedMovement` on Windows/macOS/ChromeOS; **accelerated on every Linux
browser**; no HID mouse reports via WebHID/WebUSB; units differ per engine;
timestamps 100 µs/1 ms coarsened (5 µs/20 µs with cross-origin isolation).

### 3.4 Pipeline taxonomy (the practical differences requested in Phase 0.2)

| Layer | Definition | Separated from the layer above by |
|---|---|---|
| **L1 Device-reported counts** | integer X/Y of a HID input report, one value per report | — (firmware processing is inside L1) |
| **L2 OS raw input events** | per-device relative deltas before pointer ballistics (evdev, Raw Input, GameInput, libinput unaccel, XI2 raw, Wayland `dx_unaccel`, macOS field 170 / GCMouse) | aggregation, zero-suppression, rotation/scale, filter drivers/plugins, absolute-mode substitution — **no acceleration** |
| **L3 OS pointer/cursor events** | post-ballistics display-space motion | acceleration curve, DPI/desktop scaling, pixel quantisation with remainder carry, edge clipping, coalescing |
| **L4 Browser pointer events** | `movementX/Y` (L4u unadjusted from L2; L4a adjusted from L3; L4p positions) | browser IPC/queues, unit conversion (DIP/CSS px/zoom), integer truncation, rAF-aligned coalescing, coarsened timestamps |
| **L5 Application-processed movement** | KineForge's transform counts → angle (sensitivity, directional gain, remainder policy, tick binning) | app maths (kept integer until the final multiply) |
| **L6 Camera rotation** | integrated yaw/pitch at a simulation tick | pitch clamp, wrap, precision, frame pacing |
| **L7 Rendered crosshair / photon** | when the view change reaches emitted light | GPU queue, present mode, compositor, scanout, panel response — measured externally only |

Labelling rules: only L1 is "device counts"; evdev and Windows Raw Input
relative packets may be `os_raw_counts`; absolute packets are never deltas;
every timestamp carries `(clock, resolution, meaning)`; interpolated stamps
are derived data; gaps are first-class events. Contamination detection:
dual-source ratio test against an L1 source (ratio ≡ 1 clean; constant ≠ 1
scaling; rising with speed = acceleration), speed-sweep invariance with a
hardware HID injector, quantisation fingerprints (non-integer values from an
integer source; µs stamps ≡ 0 mod 1000 ⇒ ms source).

**Local machine facts (read-only):** Glorious receiver `258a:2031` is USB
full-speed, `bInterval` 1 ms (≤1 kHz), 16-bit signed X/Y, no Resolution
Multiplier; libinput profile **adaptive**, speed 0; `event7` unreadable
(not in `input`), but `XI_RawMotion` and `/dev/hidraw2` (mouse reports) are
readable — a dual-source validation (HID truth vs X11 raw vs browsers) is
possible today without any permission change.

## 4. GUI

| Option | Latest (date) | Licence | Native | Web | Accessibility | Limitations |
|---|---|---|---|---|---|---|
| **egui / eframe / egui-wgpu** | 0.37.0 (2026-10-15) | MIT OR Apache-2.0 | ✅ (wgpu 30, winit 0.30) | ✅ wasm | Native: AccessKit (UIA, macOS, AT-SPI). **Web: speech-synthesis only** (AccessKit web adapter "planned") | immediate-mode relayout per frame (cheap at HUD scale); adds wasm size |
| egui_plot | 0.38.0 (2026-10-19) | MIT OR Apache-2.0 | ✅ | ✅ | via egui | fine for session charts |
| iced | 0.14.0 (2025-12-07) | MIT | ✅ (wgpu **27**) | listed | none (#552) | "experimental"; wgpu 27 ≠ 30 → two wgpu copies |
| Slint | 1.18.1 (2026-09-21) | GPL-3.0 / royalty-free with attribution / commercial | ✅ | canvas/WebGL, "not recommended for general-purpose web applications", no screen reader | AccessKit native | attribution requirement or GPL |
| Dioxus | 0.7.10 (2026-07-30) | MIT OR Apache-2.0 | webview (+ experimental native renderer) | DOM | browser-native | native = a webview beside the wgpu window: a second UI runtime |
| Xilem / Masonry | 0.4.0 (2025-10-29) | Apache-2.0 | ✅ | xilem_web | AccessKit | "experimental" |
| DOM (HTML/CSS/TS) around the canvas | — | — | needs a webview | best | best on web | two UI codebases or a webview on native |

Charting: egui_plot in-app; for rich browser dashboards later, uPlot (1.6.32, MIT), ECharts (6.1.0, Apache-2.0), Chart.js (4.6.1, MIT). `plotters` has not released since 2024-09 (stagnating).

**Shared vs separate UI.** The real-time trainer and the research/analytics
tools have different needs, but the settings, calibration and session-result
views are the same on native and web. egui gives one codebase for both and
renders in the same wgpu pass as the scene (no second runtime; per-frame cost at HUD scale
expected to be small but unmeasured — PERFORMANCE_PLAN N1/W5 report it). The gap is web screen-reader
accessibility. A DOM shell for menus on the web only is the remedy if
accessibility becomes a v1 requirement (decision requested in STACK_DECISION.md).
A separate analysis application is **not** needed initially: `kf-cli` plus
Parquet export covers heavy offline analysis in Python/R/DuckDB.

## 5. Mathematics and simulation

| Crate | Version (date) | Licence | Use in KineForge |
|---|---|---|---|
| **glam** | 0.34.3 (2026-11-08) | MIT OR Apache-2.0 | **f64 types (`DVec3`, `DQuat`, `DMat4`) in the deterministic core** — scalar-only implementation; f32 SIMD types only in the renderer. Enable the `libm` feature for the core |
| nalgebra | 0.35.0 | Apache-2.0 | not needed (generic-heavy; no glam 0.34 bridge) |
| parry3d / rapier3d | 0.31.1 / 0.36.1 | Apache-2.0 | **not needed**: hit-scan against spheres/capsules/boxes is ≈100 lines of exact arithmetic. Fallback only for mesh hitboxes (with `enhanced-determinism`) |
| rand | 0.10.3 | MIT OR Apache-2.0 | **not in the core**: `StdRng`/`SmallRng` opt out of reproducibility; 0.9 changed `Uniform`/`shuffle`/`choose` output; 0.10 swapped `rand_chacha` for `chacha20` |
| rand_xoshiro / rand_pcg | 0.8.1 / 0.10.2 | MIT OR Apache-2.0 | optional; generator algorithms are fixed specs. Own ≈40-line generator + samplers with golden-value tests is preferred |
| **libm** | 0.2.16 | MIT | **pinned `=0.2.16`** for every transcendental in the sim path (pure-Rust musl/FreeBSD port; architecture overrides only for exactly-rounded ops, so non-NaN results are **inferred** from source to be identical on x86-64, aarch64 and wasm32 — not documented by the crate; confirmed only by the E10 golden-hash test) |
| statrs | 0.19.1 | MIT | analysis only (`default-features = false` avoids pulling nalgebra + rand) |
| argmin / levenberg-marquardt | 0.11.0 / 0.15.0 | MIT OR Apache-2.0 / MIT | calibration fitting; note argmin pulls rand 0.9, levenberg-marquardt pulls nalgebra 0.34 — choose one, or hand-write a small Gauss–Newton (decide at implementation) |
| biquad, rustfft | 0.6.0, 6.4.1 | MIT OR Apache-2.0 | analysis filters/spectra (rustfft results may differ by SIMD backend — UNVERIFIED — so never in scored paths) |
| fixed | 1.31.0 | MIT/Apache-2.0 (MSRV 1.93) | **not needed**: ODD's fixed-point is ported with plain integer ops |

### 5.1 Floating-point determinism (RFC 3514 and std docs)

- Exactly IEEE-specified in Rust: `+ − × ÷`, `%`, `sqrt`, `mul_add`, comparisons, casts, `floor/ceil/round/trunc`. No implicit FMA contraction (contraction would violate RFC 3514's per-operation guarantees).
- **Non-deterministic by documentation**: `sin`, `cos`, `tan`, `exp`, `ln`, `powf`, `powi`, `atan2`, `hypot`, … ("varies by platform, Rust version, and can even differ within the same execution"); `algebraic_*` methods; `mul_add_relaxed`/`fmuladd`.
- NaN sign/payload bits are not deterministic (and on wasm NaN generation is non-deterministic) → never hash or serialise NaN bits; reject or canonicalise.
- 32-bit x86 (x87) breaks IEEE semantics → no 32-bit x86 builds.
- `+relaxed-simd` on wasm makes `fmuladd` lower to one-or-two-rounding ops → never enable it for the core.

**Exact-reproducibility requirements**

| Path | Bit-exact? | How |
|---|---|---|
| counts → yaw/pitch | yes | i64 accumulated counts × f64 constant, computed fresh each tick (never accumulate float increments) |
| directional gain (ODD) | yes | integer port of ODD fixed-point (OPEN_DIRECTIONAL_DPI_AUDIT.md) |
| target spawning/motion | yes | own seeded PRNG + samplers; motion as functions of tick; trig via `libm` |
| hit tests | yes | f64 basic ops + `sqrt` |
| official scores | yes | basic ops + `libm`, fixed sequential reduction order, `scoring_version` |
| rendering, interpolation, effects | no | f32 SIMD, GPU |
| offline analytics (filters, FFT, fits) | tolerance-based | declared tolerances |

Enforcement: clippy `disallowed_methods` for std transcendentals, `algebraic_*`
and `mul_add_relaxed` in core crates; no `-C target-cpu=native` for release
builds; no `HashMap` iteration in sim logic; a CI golden-replay hash compared
on x86-64 Linux, aarch64 macOS and wasm32.

## 6. Telemetry and data formats

### 6.1 Candidates (2026-11-13)

| Crate / format | Version | Licence | Notes |
|---|---|---|---|
| arrow / arrow-ipc | 60.1.0 | Apache-2.0 | IPC stream readable up to the last complete message; one schema per stream; builds for wasm32 in upstream CI |
| **parquet** | 60.1.0 | Apache-2.0 | derived tables; builds for wasm32 in CI; `zstd` codec is C (needs clang for wasm), `snap`/`lz4_flex`/`brotli`/`flate2`(zlib-rs) are pure Rust. parquet-wasm reports ≈1.2 MB brotli for an all-codecs bundle, 456 KB read-only minimal |
| rusqlite / sqlite-wasm-rs | 0.42.0 / 0.6.3 | MIT | C dependency on both targets; browser OPFS `sahpool` VFS is Worker-only; concurrent connections only within one thread or via `pauseVfs`/`unpauseVfs`; one VFS instance per origin/directory |
| serde_json | 1.0.154 | MIT OR Apache-2.0 | manifests |
| postcard | 1.1.3 | MIT OR Apache-2.0 | stable documented wire format; no maintained Python reader (UNVERIFIED) |
| rmp-serde, prost, flatbuffers, capnp, ciborium | various | various | viable; codegen tools (protoc/flatc/capnp) or slow (ciborium, no release since 2024-01) |
| **bincode** | 3.0.0 | MIT | **unmaintained** (RUSTSEC-2025-0141); 3.0.0 is a `compile_error!` tombstone — do not use |
| crc32fast | 1.5.5 | MIT OR Apache-2.0 | CRC-32 (IEEE), same as Python `zlib.crc32` |
| zip | 9.5.1 | MIT | export bundles (stored members) |

### 6.2 What decides the format

Published serialisation benchmarks (rust_serialization_benchmark, updated
2026-11-05) show even serde_json at ≈0.4 GB/s — more than 400× KineForge's
worst-case raw rate (≈0.85 MB/s: three 8 kHz sources + frames + simulation
records, DATA_SPECIFICATION.md §6.6). **Throughput
does not decide.** What decides: crash recovery at record granularity, total
ordering across event kinds, append-only writes that work identically on a
native file and an OPFS sync handle, zero codegen tooling, a tiny wasm
footprint, and a stdlib-only reader in other languages. General-purpose
codecs compress raw float data poorly (6.0 MB → 5.35 MB with zstd in the
benchmark's mesh set); columnar delta encodings (Parquet) are what shrink
telemetry — which is why Parquet is used for derived/exported tables, not
the live log.

### 6.3 Storage architecture (adopted in DATA_SPECIFICATION.md)

- **Raw log** (`raw.kflog`): custom append-only chunked format — fixed-layout little-endian records, per-chunk length + kind + CRC-32; recovery = valid prefix up to the last `commit` chunk, recorded in a `raw.recovered.json` sidecar — the raw file is never modified (DATA_SPECIFICATION.md §5.4); one file per session (total order across kinds). Dependencies: std + `crc32fast`.
- **Derived tables**: Parquet (zstd natively; snappy/lz4_raw on web unless clang-for-wasm is available).
- **Manifest**: JSON.
- **Export**: ZIP with stored members containing the manifest, raw log, Parquet tables, a byte-level `FORMAT.md` (text); the ≈40-line stdlib-only Python reader is published separately with its SHA-256, never shipped inside bundles.
- **No SQLite in v1** — add only when scanning manifests to build the session index is measurably slow.
- Browser: the log is written from a dedicated Worker through an OPFS `FileSystemSyncAccessHandle` (exclusive lock, synchronous `write`/`flush`); always offer export because OPFS data can be evicted.

## 7. Build, distribution, profiling

| Item | Version / date | Finding |
|---|---|---|
| Rust stable | 1.100.0 (2026-11-12) | 6-week cadence. Tier 1 with host tools: `x86_64/aarch64-pc-windows-msvc`, `x86_64/aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`; `x86_64-apple-darwin` Tier 2; `wasm32-unknown-unknown` Tier 2 |
| Pinning | — | `rust-toolchain.toml` with an exact channel, targets, clippy/rustfmt; committed `Cargo.lock`; `--locked` everywhere |
| Reproducible paths | — | Cargo `trim-paths` still unstable; use rustc `--remap-path-prefix`/`--remap-path-scope` |
| Supply chain | cargo-deny 0.20.2, cargo-audit 0.23.0, cargo-vet 0.10.3, cargo-auditable 0.7.7, cargo-cyclonedx 0.5.10, cargo-about 0.9.3 | all active |
| Web tooling | wasm-bindgen 0.2.130 (now under the `wasm-bindgen` GitHub org), wasm-pack 0.15.1, trunk 0.22.1 (MSRV 1.96.1), binaryen version_133 | the `wasm-opt` crate is stale (2024); use binaryen release binaries. wasm-bindgen-cli must match the library version exactly |
| Release orchestration | cargo-dist 0.34.0 (2026-11-02) | active; archives, shell/powershell/MSI/homebrew installers; **no .app/.dmg/Flatpak/deb** |
| App packaging | cargo-packager 0.11.8 (2025-11-27); cargo-bundle 0.12.0 | packager: dmg/app, deb, AppImage, NSIS/MSI, signing fields, but no release for ~1 year; bundle: Windows/iOS "experimental" |
| Flatpak | — | flatpak-builder + flatpak-cargo-generator (not checked) |
| macOS signing | — | Developer ID + hardened runtime + notarisation on macOS runners (Apple page not machine-readable — UNVERIFIED details); rcodesign last released 2024-11 (stale fallback) |
| Windows signing | — | Azure Artifact Signing GA since 2026-01, but individual identities limited to US/Canada (secondary source); otherwise OV/EV certificate in an HSM/cloud KMS |
| Cross-compilation | cargo-xwin 0.23.1, cargo-zigbuild 0.24.1 | Windows MSVC from Linux possible (MSVC licence acceptance); macOS must be built on macOS. **Policy: build and test each OS natively in CI**; cross-compile only for quick checks |
| CI runners (GitHub-hosted) | — | `ubuntu-24.04`/`-arm`, `windows-2025`, `windows-11-arm`, `macos-15`/`macos-26` (arm64), `macos-15-intel` — every target can be built and tested natively |
| Profiling | `profiling` 1.0.18 facade; tracy-client 0.19.0 (matches Tracy 0.14.1 — Tracy 0.15 released 2026-11-05 not yet supported; pin the GUI); puffin 0.20.0 | Tracy for native deep dives; in-app counters for every session |
| Benchmarks | criterion 0.8.2 (now `criterion-rs` org); divan 0.1.21 (low activity since 2025-04); **gungraun 0.20.0** (formerly iai-callgrind) | criterion for wall-time, gungraun for deterministic instruction counts in Linux CI |
| Flamegraphs | cargo-flamegraph 0.6.15 | active |

## 8. Name check — "KineForge"

Free on crates.io (`kineforge`, `kineforge-*`), npm, PyPI; no GitHub user/org
`kineforge`. At least nine small unrelated 2026 projects use the name,
including a company-backed "Kineforge AI/ML model for robotics" launch
(Product Hunt, 2026-06-21). `kineforge.com` is registered (hosting
placeholder only). USPTO/EUIPO could not be queried programmatically —
**manual trademark search required**. Risk: **medium** (not a legal
opinion). If kept: reserve the crate/npm/PyPI names and GitHub org early and
pick a non-.com domain.

## 9. Verification log

An independent adversarial pass (8 verifier agents, 2026-11-14) re-checked 43
decisive claims against primary sources (raw results:
`research/notes/verification-1.json`): **36 confirmed, 7 partly correct, 0
refuted.** Corrections applied in these documents:

| Claim | Correction |
|---|---|
| Chrome WebGPU matrix | Android: 121 for ARM/Qualcomm/Intel on Android 12+, 139 for Imagination on Android 16+, others TBD; Linux only Intel Gen12+ (144) and NVIDIA ≥535.183.01 on Wayland (147); Windows ARM64 behind a flag |
| Firefox WebGPU matrix | Apple Silicon only on macOS (Intel Macs: Nightly); Android flag-only; no WebGPU in service workers |
| OPFS sync access handles | Baseline *widely* available only since Sept 2025; synchronous `flush/getSize/truncate/close` from Chrome 108, Firefox 111, Safari 16.4 |
| Timer resolution | 100 µs / 5 µs is Chromium's choice; Firefox 1 ms + jitter / 20 µs (COI); Safari (WebKit `Performance.cpp`) 1 ms non-isolated, **20 µs with COI** (`highTimePrecision()`); confirm on hardware |
| sqlite-wasm `opfs-sahpool` | concurrency limited (same-thread connections or explicit pause/unpause), one VFS instance per origin/directory |
| glam determinism | only 4-wide dots (Vec4/Quat) differ in summation order across SSE2/NEON/scalar; NEON uses FMA **unconditionally** in quaternion multiply/slerp; f64 types scalar-only — conclusion (f64 only in the core) strengthened |
| libm | cross-platform bit-identity is inferred from source (arch overrides only for exactly-rounded ops), not documented by the crate |

Also directly reproduced by the lead researcher: ODD's direction-smoothing
window saturation (OPEN_DIRECTIONAL_DPI_AUDIT.md §6).

## 10. Key sources (accessed 2026-11-13/14)

Full lists with file/line references: `research/notes/rust_render_ui.md` §F,
`rust_math_data.md`, `input.md` §11 and `_windows.md`/`_browser.md` sources.

- crates.io API: `https://crates.io/api/v1/crates/<name>` (all version/licence/MSRV data)
- wgpu v30.0.2: https://github.com/gfx-rs/wgpu/blob/v30.0.2/wgpu/src/backend/webgpu.rs (Fifo-only web), https://github.com/gfx-rs/wgpu/blob/v30.0.2/wgpu-types/src/surface.rs (present modes, frame latency), https://github.com/gfx-rs/wgpu/blob/v30.0.2/wgpu/src/util/init.rs (WebGPU detection), https://github.com/gfx-rs/wgpu/blob/v30.0.2/wgpu-hal/src/gles/adapter.rs; issues https://github.com/gfx-rs/wgpu/issues/2869, https://github.com/gfx-rs/wgpu/issues/9856, https://github.com/gfx-rs/wgpu/issues/9937, https://github.com/gfx-rs/wgpu/issues/8354
- winit v0.30.13: https://github.com/rust-windowing/winit/blob/v0.30.13/src/event.rs; issues https://github.com/rust-windowing/winit/issues/1194 (timestamps), https://github.com/rust-windowing/winit/issues/4581 (macOS accelerated), https://github.com/rust-windowing/winit/pull/3609 (high polling), https://github.com/rust-windowing/winit/issues/2884 (RDP absolute); 0.31 changelog https://github.com/rust-windowing/winit/blob/master/winit/src/changelog/v0.31.md
- Bevy 0.20.1 features: https://github.com/bevyengine/bevy/blob/v0.20.1/docs/cargo_features.md
- egui wasm size: https://github.com/emilk/egui/issues/5889; AccessKit: https://github.com/AccessKit/accesskit
- Slint licences: https://github.com/slint-ui/slint/blob/master/LICENSE.md; Slint web: https://docs.slint.dev/latest/docs/slint/guide/platforms/web/
- SDL raw macOS: https://github.com/libsdl-org/SDL/pull/14741
- Float semantics: https://rust-lang.github.io/rfcs/3514-float-semantics.html; https://doc.rust-lang.org/std/primitive.f64.html
- rand reproducibility: https://rust-random.github.io/book/crate-reprod.html; changelog https://github.com/rust-random/rand/blob/master/CHANGELOG.md
- bincode advisory: https://rustsec.org/advisories/RUSTSEC-2025-0141.html
- arrow-rs/parquet: https://github.com/apache/arrow-rs (parquet/Cargo.toml, CI workflows); parquet-wasm: https://github.com/kylebarron/parquet-wasm
- Serialization benchmark: https://github.com/djkoloski/rust_serialization_benchmark
- Windows Raw Input: https://learn.microsoft.com/en-us/windows/win32/api/winuser/ns-winuser-rawmouse, https://learn.microsoft.com/en-us/windows/win32/inputdev/about-raw-input, https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerrawinputdevices; background throttling https://blogs.windows.com/windowsdeveloper/2023/05/26/delivering-delightful-performance-for-more-than-one-billion-users-worldwide/; Godot high-polling https://godotengine.org/article/fixing-high-polling-rate-mice-on-windows/; exclusive mouse collections https://learn.microsoft.com/en-us/windows-hardware/drivers/hid/top-level-collections-opened-by-windows-for-system-use
- Linux kernel input (v7.4-rc2 `97d73c8`): https://github.com/torvalds/linux/blob/97d73c8251226cf27cc6656b4c542869bb8c69fa/drivers/input/evdev.c, `drivers/hid/hid-input.c`, `Documentation/hid/hidraw.rst`
- libinput: https://gitlab.freedesktop.org/libinput/libinput/-/raw/main/src/libinput.h; normalisation doc https://gitlab.freedesktop.org/libinput/libinput/-/blob/main/doc/user/normalization-of-relative-motion.rst
- XI2 protocol: https://gitlab.freedesktop.org/xorg/proto/xorgproto/-/raw/master/specs/XI2proto.txt
- Wayland relative pointer: https://wayland.app/protocols/relative-pointer-unstable-v1
- Apple: https://developer.apple.com/documentation/gamecontroller/gcmouseinput, IOHIDFamily https://github.com/apple-oss-distributions/IOHIDFamily
- Rust platform support: https://doc.rust-lang.org/stable/rustc/platform-support.html; GitHub runners: https://docs.github.com/en/actions/reference/runners/github-hosted-runners
- cargo-dist: https://axodotdev.github.io/cargo-dist/book/installers/index.html; cargo-packager: https://github.com/crabnebula-dev/cargo-packager; tracy-client: https://github.com/nagisa/rust_tracy_client; gungraun: https://github.com/gungraun/gungraun
