# KineForge — Rust rendering / windowing / GUI / build-tooling research

Access date for every source below: **2026-11-13** (unless a different date is stated). Versions are from the crates.io API (`https://crates.io/api/v1/crates/<name>`: `max_stable_version`, version `created_at`, `license`, `rust_version`, `recent_downloads`) and from `gh api repos/<o>/<r>` (pushed_at, open_issues_count incl. PRs, archived). "UNVERIFIED" = could not confirm from a primary source in this session. Raw API dumps are in `research/raw/`.

---

## 0. Decisive findings (TL;DR)

1. **wgpu 30.0.2 (2026-11-12) is the only renderer that covers every target with first-class backends**: DX12/Vulkan (Win), Vulkan/GL (Linux), Metal (macOS), WebGPU **and** WebGL2 on `wasm32-unknown-unknown`. The ecosystem has settled on **wgpu 30 + winit 0.30** (Bevy 0.20, egui 0.37, Slint 1.18, vello 0.12 and kiss3d 0.47 all depend on `wgpu ^30` and `winit ^0.30`). The outlier is iced 0.14, which still uses `wgpu ^27`.
2. **WebGL2 fallback is mandatory for the browser build.** Per the gpuweb wiki (edited 2026-10-02): Firefox Linux and Android have no WebGPU in stable. Chrome Linux enables it only for Intel Gen12+ (since 144) and NVIDIA-on-Wayland (since 147). Chrome on Windows ARM64 needs a flag. wgpu handles this in one binary via `wgpu::util::new_instance_with_webgpu_detection`, but enabling `webgl` pulls **naga into the wasm**. The egui team measured +2.4 MiB (5.9 → 8.3 MiB demo) for wgpu+WebGL vs glow.
3. **Web presentation is FIFO-only in wgpu.** The WebGPU backend reports `present_modes: [Fifo]` and panics on Mailbox/Immediate. GLES on non-Windows also reports Fifo only. `desired_maximum_frame_latency` is ignored on GL. So the web build is vsync/rAF-paced: a 144–360 Hz *render* rate on web depends on the display, while *input* sampling can still be high-rate.
4. **winit has no input-correctness guarantees suitable for a mouse-research platform.** It has no event timestamps (issue #1194, open since 2019). On macOS `DeviceEvent::MouseMotion` uses NSEvent (accelerated) deltas; the GCMouse proposal (#4581) is open. The Windows high-polling-rate fix (#3609) is still an open PR. Cursor grab: `Locked` is not implemented on X11, and `Confined` is not implemented on macOS or Web. Web raw (`unadjustedMovement`) pointer-lock exists only in **0.31 (beta.4, 2026-10-24; not stable)**, and is disabled on Chrome/Linux. ⇒ **Own the input layer**: build per-platform raw-input backends with OS timestamps, and keep winit for windows and the event loop only.
5. **SDL3 has the better native input** (GCMouse raw on macOS since SDL PR #14741, merged 2026-03-22; SDL 3.4.20 released 2026-11-03), but its web path is **Emscripten only**. wgpu's WebGPU backend is compiled out on `target_os="emscripten"` (wgpu `build.rs` cfg), so SDL3 on web means GLES only. The Rust `sdl3` 0.22.1 README says "Expect some bugs and missing features."
6. **Bevy 0.20.1 is a poor fit as the whole stack.** Its MSRV is 1.97.1 (it tracks recent stable). It ships a breaking release every ~4–5 months (0.18 Jan, 0.19 Jun, 0.20 Oct 2026). WebGPU and WebGL2 need **separate wasm builds** ("you won't be able to run Wasm builds with WebGL2, only with WebGPU"). Pipelined rendering overlaps simulating frame N+1 with rendering frame N (it can be disabled). bevy_ecs and bevy_math can be used standalone, but we don't need them.
7. **GUI: egui 0.37 + egui-wgpu** is the only option that is mature, wgpu-30-native, shared native+web, cheap at high refresh, and has native a11y via AccessKit. Its gap is that **web a11y is speech-synthesis only**: the AccessKit web adapter is "planned, not released". Slint's web build is canvas/WebGL with no screen reader, and Slint's own docs say it is "not recommended for building general-purpose web applications". iced and Xilem self-describe as "experimental".
8. **Packaging**:
   - cargo-dist 0.34.0 (2026-11-02) is active, but it has no .app/.dmg/Flatpak/deb ("future work").
   - cargo-packager 0.11.8 does dmg/app/NSIS/MSI/deb/AppImage/pacman and has macOS signing/notarization and Windows signing fields. Its last release was **2025-11-27**.
   - macOS builds must happen on macOS runners.
   - GitHub has hosted runners for every target: `ubuntu-24.04-arm`, `windows-11-arm`, `macos-15`/`macos-26` (arm64) and `macos-15-intel`.
9. **Toolchain**: Rust stable is **1.100.0 (2026-11-12)**, on a 6-week cadence. Cargo `trim-paths` is **still unstable**; the stable rustc flags are `--remap-path-prefix` and `--remap-path-scope`.

---

## A. Rendering / windowing / engines

### A.1 Candidate table

| Crate | Latest stable (date) | License | MSRV | Platforms / backends | Wasm (WebGPU / WebGL2) | Maintenance signals | Key limitations for KineForge |
|---|---|---|---|---|---|---|---|
| **wgpu** | 30.0.2 (2026-11-12) | MIT OR Apache-2.0 | 1.87 (policy: ≤ stable−3, bump = breaking) | Vulkan ✅ Win/Linux/Android; Metal ✅ mac/iOS; DX12 ✅ Win; GL 3.3+/GLES 3.0+ 🆗; MoltenVK/ANGLE optional | ✅ WebGPU (first-class); 🆗 WebGL2 (downlevel, naga WGSL→GLSL); wasm64 support unreleased | 18.9M recent DL; repo pushed 2026-11-13; 1343 open issues+PRs; breaking release every ~3 months (27 Oct-25, 28 Dec-25, 29 Mar-26, 30 Jul-26) | No per-frame presentation feedback (#2869 open since 2022; #9856 Metal-first proposal open). v30 present-time regression report #9937 (open). Vulkan-on-Windows swapchain issues (#8354). Unreleased fix #10714: Vulkan/Windows waited on the acquire fence in Immediate/Mailbox, adding latency. Web: FIFO only. Timestamp queries absent on WebGL2. |
| **winit** | 0.30.13 (2026-03-02); 0.31.0-beta.4 (2026-10-24) | Apache-2.0 (only) | 1.70 (0.30); 0.31 bumps to 1.86 | Win32, AppKit, X11, Wayland, Android, iOS, Web, Redox | Web backend: canvas + DOM events; pointer lock; `getCoalescedEvents` used | 16.7M recent DL; pushed 2026-11-13; 748 open; 0.31 milestone 25 open / 61 closed, no due date | No event timestamps (#1194). macOS raw motion is accelerated (#4581). High-poll-rate Windows perf (#3609 open PR; #3689). Grab gaps (below). Windows grab bug #4473 (open). Exclusive-fullscreen panic #4737 (open). FEATURES.md is stale vs code (it says Wayland raw ❌, but the code uses `zwp_relative_pointer` unaccelerated deltas). |
| **Bevy** | 0.20.1 (2026-10-28) | MIT OR Apache-2.0 | 1.97.1 | wgpu 30 + winit 0.30 | `webgl2` (in `default_platform`) **or** `webgpu` (overrides webgl2): two builds needed | 2.09M recent DL; 3556 open; 49.7k★; ~4–5 month breaking cadence | Heavy dependency tree (bevy_internal: 15 required + 43 optional direct deps). Pipelined rendering adds a frame of overlap. Inherits winit input gaps. Wasm size not published: **UNVERIFIED**, measure. |
| **bevy_ecs / bevy_math** | 0.20.1 | MIT OR Apache-2.0 | 1.95 / 1.94 | pure Rust | yes | same as Bevy | Usable standalone (bevy_math = glam + 3 deps; bevy_ecs = 18 direct deps), but they churn with Bevy releases. |
| **sdl3** (Rust) / **sdl3-sys** | 0.22.1 (2026-11-09) / 0.7.3+SDL-3.4.20 (2026-11-04) | MIT / Zlib (SDL: Zlib) | — / 1.85 | SDL3: Win/mac/Linux/iOS/Android (+ others) | **Emscripten only** (`wasm32-unknown-emscripten`, Tier 2). wgpu on emscripten = GLES only (WebGPU backend cfg'd out) | sdl3: 115k recent DL, 405★, 30 open; sdl3-sys tracks SDL point releases; features incl. `build-from-source-static` | README: "Expect some bugs and missing features". Second platform layer needed for wasm32-unknown-unknown. C toolchain in the build. |
| **macroquad / miniquad** | 0.4.16 (2026-07-30) / 0.4.11 (2026-06-03) | MIT OR Apache-2.0 | — / 1.70 | miniquad: GL2/GL3/GLES2/3, Metal (mac/iOS) | README: "WebGL 1"; own JS loader (`gl.js`), not wasm-bindgen | repos last pushed Aug/Jul 2026; 349/197 open | No WebGPU, no DX12/Vulkan. README may be stale (badge 0.3.13). Bespoke JS glue conflicts with the wasm-bindgen ecosystem. |
| **Fyrox** | 1.0.1 (2026-03-28) | MIT | 1.87 | GL via `glow` + `glutin` (fyrox-graphics-gl deps) | WebGL2 via glow (browser demos exist) | 6.2k recent DL; repo active 2026-11-13; no release in 7.5 months | Full editor-centric engine. GL only. Overkill. |
| **ash** | 0.38.0+1.3.281 (**2024-04-01**) | MIT OR Apache-2.0 | 1.69 | Vulkan only (mac via MoltenVK) | ✗ | 19M recent DL (as a transitive dep); repo pushed 2026-11-04; no release for 2.5 years | No web, no DX12/Metal. Unsafe. |
| **vulkano** | 0.35.2 (2025-08-16) | MIT OR Apache-2.0 | 1.75 | Vulkan only | ✗ | pushed 2026-11-04 | Same as ash, plus a safety-wrapper overhead. |
| **blade-graphics** | 0.9.0 (2026-09-06) | MIT | 1.92 | Vulkan (Win/Linux/Android), Metal, GLES "at a basic level" (wasm) | WebGL2 basic; **no WebGPU** | 816★; pushed 2026-11-13 | No DX12. Windows only via Vulkan. |
| **three-d** | 0.19.0 (2026-04-17) | MIT | — | glow (OpenGL/GLES) | WebGL2 | 1.7k★; last push 2026-06-24 | GL only. Small community. |
| **kiss3d** | 0.47.0 (2026-10-04) | BSD-3-Clause | — | now on wgpu 30 + winit 0.30 (+ egui 0.36) | via wgpu | 1.8k★ | A "simple viewer" API. Pins egui 0.36 (vs 0.37). Not a frame-pacing-oriented design. |
| **rend3** | 0.3.0 (2022-02-12) | MIT/Apache/Zlib | 1.57 | — | — | **repo archived** | Dead. Excluded. |

Direct dependency counts are taken from `/api/v1/crates/<c>/<v>/dependencies`, "normal" kind, across all targets. They are direct only; transitive counts are not available without `cargo tree`.

| Crate | Required | Optional |
|---|---|---|
| wgpu | 15 | 9 |
| winit | 30 | 14 |
| bevy_internal | 15 | 43 |
| egui | 11 | 5 |
| eframe | 22 | 11 |
| egui-wgpu | 10 | 2 |
| iced | 8 | 4 |
| slint | 11 | 11 |
| miniquad | 4 | 0 |
| macroquad | 6 | 3 |
| blade-graphics | 29 | 0 |
| sdl3 | 3 | 8 |

### A.2 wgpu specifics (verified against v30.0.2 source)

- **Backends per platform** (README v30.0.2 "Supported Platforms"). Vulkan ✅ on Windows and Linux/Android, 🌋 MoltenVK on macOS. Metal ✅ on macOS/iOS. DX12 ✅ on Windows. OpenGL 🆗: GL 3.3+ on Windows, GLES 3.0+ on Linux/Android, 📐 ANGLE on macOS, WebGL2 on web. WebGPU ✅ on web.
  - Without the `webgl` feature, wgpu on web "relies on the browser's own WebGPU implementation. WGSL shaders are simply passed through". In that case naga is not needed for shader translation.
  - With `webgl`, "uses Naga to translate WGSL programs into GLSL".
- **Runtime WebGPU→WebGL2 fallback**: `wgpu::util::new_instance_with_webgpu_detection` (async). Its docs say it should be preferred "if you want to target WebGPU and automatically fall back to WebGL".
- **Present modes** (`wgpu-types/src/surface.rs` L28–105):
  - `Fifo` is supported on "All platforms" and is the default.
  - `FifoRelaxed` lists "Supported on: AMD on Vulkan".
  - `Immediate` lists "Most platforms except older DX12 and Wayland".
  - `Mailbox` lists "DX12 on Windows 10, NVidia on Vulkan and Wayland on Vulkan".
  - `AutoNoVsync` falls back Immediate → Mailbox → Fifo.
- **Web**: `webgpu.rs` L4175 sets `present_modes: vec![Fifo]`, and L4226–4228 panic with "Only FIFO/Auto* is supported on web".
- **GLES**: `gles/adapter.rs` L1371–1374 reports Fifo and Immediate on Windows, and **Fifo only elsewhere** (`//TODO`).
- **`desired_maximum_frame_latency`** (surface.rs L935–968). Default is 2.
  - "Choose `1` to minimize latency above all else… CPU and GPU cannot run in parallel."
  - Per-backend mapping: Vulkan uses swapchain images = N+1. DX12 calls `IDXGISwapChain2::SetMaximumFrameLatency(N)`. Metal sets `maximumDrawableCount` = N+1. **OpenGL: ignored**.
  - Interactions: DX12+Mailbox caps fps at N×Hz. Vulkan/Metal+Mailbox with N=2 caps at 2×Hz.
  - DX12 waitable-object skip option is in `Dx12BackendOptions` (PR #7400, merged 2025).
- **Timestamp queries**:
  - Native backends expose `TIMESTAMP_QUERY*`.
  - GLES enables them only when `GL_ARB_timer_query` is present (desktop GL), so **WebGL2 has none**.
  - On WebGPU they are passed through when the browser exposes `timestamp-query`. `write_timestamp` on encoders needs `TIMESTAMP_QUERY_INSIDE_ENCODERS`, which is "available on all native backends but not on WebGPU" (CHANGELOG).
  - Browser timestamp quantization (Chrome) is UNVERIFIED.
- **Presentation feedback / input-to-photon**: wgpu has no API for actual present time. #9856 states: "applications cannot measure frame-specific input-to-display latency". `get_presentation_timestamp` is INVALID on GLES. For latency research, plan `as_hal` escapes (DXGI frame statistics, `VK_KHR_present_wait`, `CAMetalDrawable.presentedTime`). UNVERIFIED feasibility per backend.
- **v30 API change**: `SurfaceTexture::present()` → `Queue::present(surface_texture)` (#9361).
- **Headless/offscreen**: an `Instance`/`Adapter`/`Device` without a `Surface`, rendering to textures, is standard wgpu usage (examples in repo). This suits deterministic render tests.
- **Shader tooling**: naga 30.0.2 (same version train). WGSL is the single source language for native and web. `naga::front::wgsl::ParseError::notes()` was added in v30. Offline validation goes through naga, or the `naga-cli` crate (not checked).
- **Graphics debuggers**: `Device::start_graphics_debugger_capture` drives RenderDoc (`StartFrameCapture`) and Xcode (`MTLCaptureManager`). PIX attaches externally to the DX12 backend; debug groups/markers via `push_debug_group` (WebGPU support was added in v29). RenderDoc latest is v1.47 (2026-10-31). RenderDoc has no Metal support (general knowledge).

### A.3 winit specifics

**Event-loop model**:
- 0.30 uses `ApplicationHandler` + `EventLoop::run_app`.
- In 0.31, `ApplicationHandler` takes `dyn ActiveEventLoop`, and `can_create_surfaces()` is split from `resumed()`.
- In 0.31, `user_event` becomes `user_wake_up`, MSRV goes to 1.86, the pointer events are overhauled (`CursorMoved`→`PointerMoved`, `DeviceEvent::MouseMotion`→`PointerMotion`), `DeviceEvent::Added/Removed/Motion` are removed, and the crate is split (winit-core, winit-win32, winit-web…).
- Source: `winit/src/changelog/v0.31.md` on master.

**Cursor grab per platform** (0.30.13 code, not FEATURES.md):

| Mode | Windows | macOS | X11 | Wayland | Web |
|---|---|---|---|---|---|
| `Locked` | ✅ (cursor flags) | ✅ via `CGAssociateMouseAndMouseCursorPosition(false)` | ❌ NotSupported | ✅ | ✅ pointer lock |
| `Confined` | ✅ | ❌ NotSupported | ✅ | ✅ | ❌ NotSupported |

Practical rule: try `Locked` and fall back to `Confined` (this is the doc example). On X11 you only get `Confined`.

**Raw motion (`DeviceEvent::MouseMotion`)**:

| Platform | Source | Raw? |
|---|---|---|
| Windows | `WM_INPUT` | raw |
| X11 | XI2 raw | raw |
| Wayland | `zwp_relative_pointer_v1` `dx_unaccel/dy_unaccel` | unaccelerated; the protocol's µs `utime` is dropped |
| macOS | NSEvent deltas | **accelerated** (#4581 open) |
| Web 0.30 | `movementX/Y` | accelerated |
| Web 0.31 | `requestPointerLock({unadjustedMovement:true})` | raw, **except Chrome on Linux** (`winit-web/src/lock.rs` L17–21, chromium bug 40833850) |

Web also uses `getCoalescedEvents()`, with Firefox-specific delta correction and no Safari coalesced support (`winit-web/src/web_sys/event.rs`). There is no `pointerrawupdate` usage (code search found none).

- **Timestamps**: none on any event (#1194; branch `madsmtm/event-time` exists, contents UNVERIFIED).
- **Maintenance**: active, but 0.31 has been in beta for 12 months (beta.1 2025-11-16 → beta.4 2026-10-24). The whole ecosystem is on 0.30.
- **Recommendation**: stay on 0.30.x until the frameworks move. Isolate winit behind a thin `platform` module so the 0.31 migration is local.

### A.4 Bevy specifics

- **Features**: `default = ["2d","3d","ui","audio"]`, and `default_platform` includes `webgl2`. The `webgpu` feature docs say: "When enabled, this feature will override the `webgl2` feature and you won't be able to run Wasm builds with WebGL2, only with WebGPU." (`docs/cargo_features.md` @v0.20.1.)
- **Pipelined rendering** (`bevy_render/src/pipelined_rendering.rs`): the render thread renders frame N while the main schedule runs N+1. The extra latency this adds is an inference: one frame of overlap. The plugin can be omitted.
- **wasm size**: no current primary figure was found. Search results are only the outdated cheatbook (0.12). **UNVERIFIED**; it must be measured.
- **ECS cost**: no primary benchmark numbers found. UNVERIFIED.

### A.5 Browser WebGPU availability (gpuweb Implementation-Status wiki, edited 2026-10-02)

| Browser | Status |
|---|---|
| Chromium | ✅ 113 on Mac/Win x64/ChromeOS. Android 12+ ARM/Qualcomm/Intel since 121. Linux: Intel Gen12+ since 144, NVIDIA-Wayland (driver ≥535.183.01) since 147, otherwise behind a flag. **Windows ARM64: flag**. |
| Firefox | ✅ Windows 141. macOS 26+ since 145, all macOS since 147. **Linux: Nightly only**. **Android: disabled**. |
| Safari | ✅ 26 on macOS/iOS/iPadOS/visionOS. |

⇒ A material share of Linux desktop users (our core audience for mouse research) will hit the WebGL2 path.

### A.6 Thin custom renderer vs engine; one renderer for all targets?

- An aim trainer needs a handful of pipelines: targets (instanced spheres/discs), a simple environment, a crosshair, HUD (egui), and debug lines. **A thin wgpu renderer is a few hundred to low thousands of LOC** (estimate). It gives full control over present mode, frame latency, and the frame loop (sample input → sim → record → present), and it avoids engine scheduling overhead and churn.
- One wgpu renderer with WGSL shaders serves native and web. Web differences are confined to:
  - FIFO-only present;
  - `downlevel_webgl2_defaults()` limits (no compute or storage buffers in the WebGL2 path; design shaders for that subset or keep a webgl2 shader variant);
  - no timestamp queries on WebGL2.
- **Ship two wasm artifacts**, chosen by a ~20-line JS loader that checks `navigator.gpu` + `requestAdapter()`:
  - `webgpu` feature only, with no naga in the wasm;
  - `webgl` feature, with naga.

  Alternatively ship one binary with both features via `new_instance_with_webgpu_detection`, accepting about +2.4 MiB (the egui measurement). Ponytail default: start with the single binary and split when size matters.
- SDL3 is worth revisiting only if a custom input shim on top of winit proves harder than a second platform layer. Keep it as a native-only fallback.

### A.7 Score inputs (1–5, higher is better) — platform+render stacks

| Stack | Input correctness | Portability incl. web | Performance / latency control | Maintainability | Modularity | Dependency risk (5 = low) | Licensing |
|---|---|---|---|---|---|---|---|
| **winit 0.30 + wgpu 30 thin renderer + own raw-input shim** | 4 (2 without the shim) | 5 | 4 (present-feedback gap) | 4 | 5 | 4 | 5 |
| SDL3 (native) + wgpu + web-sys/winit on web | 5 native / 3 web | 3 | 4 | 3 | 4 | 3 (C build, young Rust bindings) | 5 |
| Bevy 0.20 (full engine) | 2 | 4 (two wasm builds) | 3 | 2 | 3 | 2 | 5 |
| macroquad/miniquad | 2 | 2 | 3 | 3 | 2 | 4 | 5 |
| Fyrox 1.0 | 2 | 3 | 3 | 3 | 2 | 3 | 5 |
| three-d / kiss3d | 2 | 4 | 3 | 3 | 3 | 3 | 5 (kiss3d BSD-3) |
| ash / vulkano | 3 | 1 | 5 (native) | 2 | 3 | 3 | 5 |
| blade-graphics | 2 | 2 | 4 | 3 | 3 | 3 | 5 |

---

## B. GUI

### B.1 Candidate table

| Crate | Latest (date) | License | MSRV | Renderer / backends | Web | A11y | Maintenance | Limitations for KineForge |
|---|---|---|---|---|---|---|---|---|
| **egui / eframe / egui-wgpu** | 0.37.0 (2026-10-15) | MIT OR Apache-2.0 | 1.95 | eframe default = **wgpu** (glow optional); egui-wgpu on `wgpu ^30`, `winit ^0.30.13` | ✅ wasm; eframe prefers WebGPU, WebGL configurable at runtime | Native: AccessKit (`accesskit` default feature). **Web: `web_screen_reader` = SpeechSynthesis only** | 9.8M recent DL; 32k★; ~monthly releases | Immediate mode re-lays-out each frame (cheap for HUD-sized UIs). wgpu backend adds wasm size (+2.4 MiB with WebGL/naga, #5889). |
| egui_plot | 0.38.0 (2026-10-19) | MIT OR Apache-2.0 | 1.95 | egui | ✅ | via egui | separate repo, 491★ | Fine for live/session charts. Less polished than JS libraries for rich dashboards. |
| **iced** | 0.14.0 (2025-12-07) | MIT | 1.88 | iced_wgpu on **wgpu ^27**, tiny-skia fallback | listed "the Web" | none (#552 open) | ~1 release/yr; README: "Iced is currently experimental software" | wgpu 27 ≠ 30 means two wgpu copies or no shared device. |
| **Slint** | 1.18.1 (2026-09-21) | GPL-3.0-only OR Slint-Royalty-free-2.0 OR Slint-Software-3.0 | 1.92 | winit + femtovg (wgpu 30)/Skia/software | canvas/WebGL; "not recommended for building general-purpose web applications"; "Accessibility features (such as screen readers) are not available" (web) | AccessKit native | 1.0M recent DL; active | See licensing quotes below. |
| **Dioxus** | 0.7.10 (2026-07-30; 0.8.0-alpha.3) | MIT OR Apache-2.0 | 1.83 | Web: **DOM**; desktop: **webview**; native WGPU renderer "experimental" (Blitz) | ✅ DOM (README claims ~50 kB hello world) | browser-native on web | 1.39M recent DL; 39.7k★ | Native = webview (WebView2/WKWebView/WebKitGTK) beside a separate wgpu window, which is a second UI runtime on native. |
| **Xilem / Masonry** | 0.4.0 (2025-10-29) | Apache-2.0 | 1.88 (README: 1.96 for main) | Vello + wgpu; Masonry uses winit; `xilem_web` backend | xilem_web | AccessKit | README: "experimental"; 3.3k recent DL | Too early. |
| **DOM UI (HTML/CSS/TS) around the wasm canvas** | — | — | — | browser | native to web | Best on web (real DOM a11y) | — | Doesn't exist on native unless you add a webview (Tauri/wry/Dioxus-desktop). Two UI codebases, or a webview runtime on native. |

**Slint licensing (quoted from `LICENSE.md` and `LicenseRef-Slint-Royalty-free-2.0`)**:
- **GPLv3**: "Permits use in open source software under GPL-compatible terms, at no cost, for desktop, mobile, and web applications, as well as for embedded systems."
- **Royalty-free**: "Permits use in proprietary desktop, mobile, and web applications at no cost. Use in embedded systems is excluded." It is free "as long as you disclose that you use Slint". The disclosure options are:
  - (a) the AboutSlint widget in an About screen "accessible from the top level menu of the Application" (or the splash screen);
  - (b) the Slint attribution badge "on a public webpage".

  Other conditions: it "does not permit the distribution of Application that exposes the APIs, in part or in total, of the Software", and the license notices must not be removed.
- **Commercial**: required for embedded, or for no disclosure.

**AccessKit** (README): released adapters are Windows UIA, macOS, Unix AT-SPI, Android and iOS. The **Web adapter is "planned"**, described as "for applications that render their own UI elements to a canvas". accesskit 0.25.2 crate metadata says MIT OR Apache-2.0, while GitHub detects BSD-3-Clause for the repo. Reason not checked; allow both in cargo-deny.

### B.2 Charting

| Library | Version (date) | License | Notes |
|---|---|---|---|
| egui_plot | 0.38.0 (2026-10-19) | MIT/Apache | in-app |
| plotters | 0.3.7 (**2024-09-08**, no release for 2+ years; repo pushed 2026-04) | MIT | backends incl. `plotters-canvas` 0.3.1 for wasm; stagnating |
| uPlot | 1.6.32 (2025-03-14) | MIT | npm. Fastest time-series option for large point counts (well-known; no benchmark run here) |
| ECharts | 6.1.0 (2026-05-19) | Apache-2.0 | npm |
| plotly.js-dist-min | 4.3.0 (2026-11-10) | MIT | npm |
| Chart.js | 4.6.1 (2026-10-22) | MIT | npm |
| charming | 0.6.0 (2025-06-17) | MIT/Apache | Rust wrapper over ECharts |

### B.3 GUI score inputs (1–5)

| Option | Shared native+web | High-refresh friendliness | A11y | Bundle impact | Maintainability | Dep risk | Licensing |
|---|---|---|---|---|---|---|---|
| **egui 0.37 (+egui_plot)** | 5 | 5 | 4 native / 2 web | 3 | 4 | 4 | 5 |
| DOM UI + TS + uPlot (web) / egui (native) | 2 (two UIs) | 4 | 5 web | 5 | 3 | 4 | 5 |
| Dioxus (DOM web + webview native) | 4 | 3 (webview beside wgpu) | 4 | 4 web / 2 native | 3 | 3 | 5 |
| Slint 1.18 | 3 (web not recommended) | 4 | 4 native / 1 web | 3 | 4 | 4 | 3 (attribution or GPL) |
| iced 0.14 | 3 | 4 | 1 | 3 | 2 | 2 (wgpu 27) | 5 |
| Xilem 0.4 | 3 | 4 | 3 | 3 | 2 | 2 | 5 |

---

## C. Build / distribution / profiling

### C.1 Toolchain and supply chain

| Item | Version / date | Notes |
|---|---|---|
| Rust stable | **1.100.0 (2026-11-12)**; 1.99.1 (2026-10-16); 1.99.0 (2026-10-01); 1.98.0 (2026-08-20) | `static.rust-lang.org/dist/channel-rust-stable.toml`, rust-lang/rust releases. 6-week cadence. |
| Target tiers | — | Tier 1 with host tools: `x86_64/aarch64-pc-windows-msvc`, `x86_64-pc-windows-gnu`, `x86_64/aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`. `x86_64-apple-darwin` is **Tier 2** with host tools. `wasm32-unknown-unknown` and `wasm32-unknown-emscripten` are Tier 2. `wasm64-unknown-unknown` is Tier 3. Source: rustc platform-support page. |
| Pinning | — | `rust-toolchain.toml` with `channel = "1.100.0"`, `targets = [...]`, `components = ["clippy","rustfmt"]`. Commit `Cargo.lock` and build with `--locked`. Set an MSRV ≥ the max of deps (egui 1.95, Bevy 1.97.1 if ever used; wgpu 1.87). |
| Reproducibility | — | Cargo `trim-paths` is **still unstable** (`cargo-features = ["trim-paths"]` in the stable docs). Use `--remap-path-prefix` and `--remap-path-scope` (stable rustc). |
| cargo-deny | 0.20.2 (2026-07-09) | MIT/Apache; active |
| cargo-audit | 0.23.0 (2026-11-10) | active |
| cargo-vet | 0.10.3 (2026-10-22) | Mozilla; active |
| cargo-auditable | 0.7.7 (2026-10-02) | embeds the dependency list in the binary |
| cargo-cyclonedx | 0.5.10 (2026-11-06) | SBOM |
| cargo-about | 0.9.3 (2026-10-26) | license notices |
| wasm-bindgen | 0.2.130 (2026-10-30) | now under the `wasm-bindgen` GitHub org |
| wasm-pack | 0.15.1 (2026-10-30) | — |
| trunk | 0.22.1 (2026-10-26) | MSRV 1.96.1 |
| binaryen (wasm-opt) | version_133 (2026-09-21) | The `wasm-opt` crate is stale (0.116.1, 2024-03); use the binaryen release binary. |

### C.2 Packaging and signing

**cargo-dist 0.34.0 (2026-11-02)**
- Active: releases 0.31 Feb, 0.32 May, 0.33 Sep, 0.33.1 Oct, 0.34 Nov 2026; commits daily.
- Installers: shell, powershell, npm, homebrew, **msi**.
- Its docs list as future work: "Docker image, Flatpak, macOS cask, macOS dmg/app, PyPI, winget".
- Good for release CI and archives. Not enough on its own for a GUI app.

**cargo-packager 0.11.8 (2025-11-27; repo pushed 2026-11-13; 38 open)**
- Formats: DMG and .app; deb, AppImage, pacman; NSIS and MSI (WiX).
- Has an updater.
- `MacOsConfig` fields: `signing_identity`, `signing_certificate` (base64 .p12, env-only), `notarization_credentials`, `entitlements`, `provider_short_name`.
- `WindowsConfig` fields: `certificate_thumbprint`, `digest_algorithm`, `timestamp_url`, `tsp`, and `sign_command` (custom; the default `signtool.exe` is Windows-only).
- Release cadence has slowed: about one year without a release.

**cargo-bundle 0.12.0 (2026-09-20)**
- Formats: deb, ios, msi, wxsmsi, osx, rpm, appimage.
- README: "iOS and Windows support is still experimental".

**Flatpak**: none of the three tools does it. Use flatpak-builder with flatpak-cargo-generator. Not checked here.

**macOS distribution**
- Requires a Developer ID signature, hardened runtime, secure timestamp, `notarytool` submit, and staple.
- Apple's notarization page is JS-rendered and could not be read, so these details are general knowledge: **UNVERIFIED this session**.
- `apple-codesign`/rcodesign (sign and notarize from Linux) last released 0.29.0 on **2024-11-29**: treat it as a stale fallback. Sign on macOS runners.

**Windows signing**
- Azure **Artifact Signing** (renamed from Trusted Signing) has been GA since Jan 2026 (devclass 2026-01-14; MS Learn quickstart). Public-trust certs for **individuals are limited to USA/Canada**, per MS Q&A June 2026 (secondary).
- Otherwise use an OV/EV cert with the key in an HSM or cloud KMS.
- The CA/B Forum max validity (~458–460 days from 2026-03-01) is **UNVERIFIED**.

**Cross-compilation**
- **Windows MSVC from Linux**: cargo-xwin 0.23.1 (2026-08-13). It downloads the MSVC CRT/SDK and requires accepting the Microsoft license.
- **windows-gnu**: via mingw.
- **cargo-zigbuild** 0.24.1 (2026-11-04): glibc-version targeting for Linux.
- **macOS**: build, sign and notarize on macOS runners. The Apple SDK licensing for non-Apple hosts is not checked here.

**GitHub-hosted runners** (docs.github.com "GitHub-hosted runners", public repos)

| Platform | Labels | vCPU / RAM |
|---|---|---|
| Linux x64 | `ubuntu-24.04`, `ubuntu-26.04` | — |
| Linux arm64 | `ubuntu-24.04-arm`, `ubuntu-22.04-arm`, `ubuntu-26.04-arm` | 4 / 16 GB |
| Windows x64 | `windows-2025`, `windows-2025-vs2026`, `windows-2022` | — |
| Windows arm64 | `windows-11-arm`, `windows-11-vs2026-arm` | — |
| macOS arm64 | `macos-15`, `macos-26`, `macos-latest`, `xcode-27` | 3 / 7 GB |
| macOS Intel | `macos-15-intel`, `macos-26-intel` | — |

⇒ Every target in the matrix can be built and tested natively in CI. Chrome on Windows ARM64 needs a flag for WebGPU, so web CI on that platform uses WebGL2.

### C.3 Profiling, benchmarking, debugging

| Tool | Version (date) | Notes |
|---|---|---|
| `profiling` (facade) | 1.0.18 (2026-05-04) | Used by wgpu/egui. Switch backends via features. |
| tracy-client | 0.19.0 (2026-08-25); -sys 0.30.1 (2026-10-20) | README compat table: 0.19.0 ↔ **Tracy 0.14.1**. Tracy **v0.15.0 released 2026-11-05** is not yet in the table, so pin the Tracy GUI to 0.14.1 (protocol must match; UNVERIFIED whether 0.15 is backward compatible). |
| puffin / puffin_http | 0.20.0 / 0.17.0 (2026-03-18) | MSRV 1.92. Embark repo pushed 2026-10-26. In-app flame view via puffin_egui (not checked). |
| criterion | 0.8.2 (2026-02-04) | Now under the `criterion-rs` org. |
| divan | 0.1.21 (**2025-04-10**) | Last code commit Apr 2025. Low activity. |
| gungraun (ex-iai-callgrind) | 0.20.0 (2026-09-26) | `iai-callgrind/iai-callgrind` redirects to `gungraun/gungraun`. iai-callgrind's last crate is 0.16.1 (2025-07). Valgrind-based, so Linux CI only; deterministic instruction counts for core sim/scoring regressions. |
| flamegraph (cargo-flamegraph) | 0.6.15 (2026-11-02) | Active. |
| RenderDoc | v1.47 (2026-10-31) | Vulkan/DX12/GL. Use with `start_graphics_debugger_capture`. |
| Xcode GPU capture | — | Via `MTLCaptureManager` in the same wgpu API. |
| PIX | — | External attach to the DX12 backend (UNVERIFIED marker fidelity). |

---

## D. Recommendation

**Stack: `kf-core` (pure Rust, no graphics/windowing deps; glam 0.34.3 for math) + `kf-input` (own raw-input backends) + `kf-render` (thin wgpu 30 renderer) + `kf-app` (winit 0.30 shell) + egui 0.37/egui-wgpu for all in-app UI, with egui_plot for charts.**

1. **Renderer-independent core.** Simulation, scoring, the input model and timing live in `kf-core` and are tested headless (gungraun/criterion). Rendering consumes a snapshot.

2. **Input layer (decisive for "input correctness").** `kf-input` exposes timestamped raw deltas from:
   - **Windows**: Raw Input on a dedicated thread with `GetRawInputBuffer` and QPC timestamps. This avoids the winit #3609/#3689 high-poll problems.
   - **Linux Wayland**: `zwp_relative_pointer_v1`, keeping the `utime` field.
   - **Linux X11**: XI2 raw events.
   - **macOS**: GCMouse (raw, as SDL3 does), with `mach_absolute_time`.
   - **Web**: `requestPointerLock({unadjustedMovement:true})`, plus `pointerrawupdate`/`getCoalescedEvents()` with `event.timeStamp`, via web-sys directly.

   winit `DeviceEvent` is the fallback. The exact OS-API details per platform need a spike: UNVERIFIED for GCMouse latency, and for Chrome/Linux unadjustedMovement support.

3. **Present policy.**
   - Native: `desired_maximum_frame_latency = 1`, a user-selectable `AutoNoVsync`/`Mailbox`/`Fifo`, and DX12 as the default backend on Windows (DXGI waitable object; avoids the Vulkan-Windows swapchain issues #8354 and the unreleased #10714 latency fix).
   - Web: Fifo, rAF-driven.

4. **Web build.**
   - `wasm32-unknown-unknown` + wasm-bindgen.
   - Start with one binary using `new_instance_with_webgpu_detection` (webgpu+webgl). Split into webgpu-only and webgl2 artifacts if the +~2.4 MiB naga cost matters.
   - Avoid compute in the WebGL2 path (downlevel limits).

5. **UI.** Use egui for settings, HUD, session analytics and dev overlays on both native and web. It ships the same code, and its immediate-mode cost is negligible at HUD scale.
   - **Known gap**: screen-reader a11y on web. If a11y on web becomes a requirement, add a thin DOM shell (menus/settings) on web only. Ponytail: skip until required.
   - Rich long-term analytics dashboards on web can use uPlot/ECharts later.

6. **Rejected**:

   | Option | Reason |
   |---|---|
   | Bevy as the full stack | churn, MSRV, two wasm builds, pipelined latency, size |
   | iced | experimental, wgpu 27, no a11y |
   | Slint | attribution/GPL licensing, web not recommended, no web a11y |
   | macroquad | WebGL1, no modern backends |
   | Fyrox / three-d | GL only |
   | ash / vulkano / blade | no web, or no DX12/WebGPU |
   | rend3 | archived |

   **SDL3** is held as a native-input fallback.

7. **Build**: rust-toolchain.toml pinned to 1.100.0; `--locked`; cargo-deny + cargo-audit in CI, cargo-auditable on release binaries; cargo-dist for archives and the release workflow; cargo-packager (or platform-native scripts) for .app/.dmg/MSI/deb/AppImage; signing on macOS and Windows runners.

---

## E. Open questions / to measure

1. **Input-to-photon latency per backend.** wgpu has no present feedback, so decide between `as_hal` escapes and an external photodiode/LDAT rig for ground truth.
2. **Web limits**: actual achievable latency and rAF rate in Chrome, Firefox and Safari at 144–360 Hz. Safari high-refresh rAF behaviour is UNVERIFIED. Chrome WebGPU timestamp quantization is UNVERIFIED.
3. **Chrome on Linux** lacks `unadjustedMovement` per winit's comment. Is raw web input on Linux impossible, or just accelerated? Measure and document it as a known limitation.
4. **Real wasm sizes** (gz/br) for wgpu+egui with webgpu-only vs webgpu+webgl. Bevy has no current published figure.
5. **wgpu 30 present regression (#9937).** Benchmark 29.0.4 vs 30.0.2 on each OS before locking in.
6. **winit 0.31** stable timing; keep the platform shim thin. Does a `madsmtm/event-time` branch land timestamps upstream?
7. **Windows signing** for an individual outside US/CA (Artifact Signing ineligible): OV in a cloud HSM vs unsigned plus a SmartScreen reputation ramp.
8. **Tracy 0.15** support in tracy-client: pin the GUI to 0.14.1 until it lands.
9. **Web a11y.** Is it a v1 requirement? It determines whether a DOM shell is needed.

---

## F. Sources (all accessed 2026-11-13)

**crates.io API**: `https://crates.io/api/v1/crates/{wgpu,naga,winit,bevy,bevy_ecs,bevy_math,sdl3,sdl3-sys,macroquad,miniquad,fyrox,ash,vulkano,blade-graphics,three-d,rend3,kiss3d,egui,eframe,egui-wgpu,egui-winit,egui_plot,iced,slint,dioxus,dioxus-native,xilem,masonry,plotters,plotters-canvas,charming,accesskit,vello,femtovg,glam,cargo-dist,cargo-packager,cargo-bundle,cargo-deny,cargo-vet,cargo-audit,cargo-auditable,cargo-xwin,cargo-zigbuild,cargo-about,cargo-cyclonedx,apple-codesign,tracy-client,tracy-client-sys,puffin,puffin_http,criterion,divan,iai-callgrind,gungraun,flamegraph,profiling,wasm-bindgen,wasm-bindgen-cli,wasm-pack,trunk,wasm-opt,glow,sdl2,fyrox-graphics-gl}`, plus `/api/v1/crates/<c>/<v>/dependencies` for the dependency counts.

**GitHub API**: `repos/<o>/<r>` for every repo in the tables; releases for gfx-rs/wgpu, rust-windowing/winit, emilk/egui, iced-rs/iced, axodotdev/cargo-dist, crabnebula-dev/cargo-packager, libsdl-org/SDL, vhspace/sdl3-rs, WebAssembly/binaryen, baldurk/renderdoc, wolfpld/tracy.

**wgpu**
- https://github.com/gfx-rs/wgpu/blob/trunk/CHANGELOG.md
- https://github.com/gfx-rs/wgpu/blob/v30.0.2/README.md
- https://github.com/gfx-rs/wgpu/blob/v30.0.2/wgpu-types/src/surface.rs
- https://github.com/gfx-rs/wgpu/blob/v30.0.2/wgpu/src/backend/webgpu.rs
- https://github.com/gfx-rs/wgpu/blob/v30.0.2/wgpu-hal/src/gles/adapter.rs
- https://github.com/gfx-rs/wgpu/blob/v30.0.2/wgpu/build.rs
- https://github.com/gfx-rs/wgpu/blob/v30.0.2/wgpu/Cargo.toml
- https://github.com/gfx-rs/wgpu/blob/v30.0.2/wgpu/src/util/init.rs
- https://github.com/gfx-rs/wgpu/blob/v30.0.2/wgpu/src/api/device.rs
- Issues and PRs:
  - https://github.com/gfx-rs/wgpu/issues/2869
  - https://github.com/gfx-rs/wgpu/issues/9856
  - https://github.com/gfx-rs/wgpu/issues/9937
  - https://github.com/gfx-rs/wgpu/issues/8354
  - https://github.com/gfx-rs/wgpu/pull/7400
  - https://github.com/gfx-rs/wgpu/pull/10714

**winit**
- https://github.com/rust-windowing/winit/blob/v0.30.13/src/window.rs
- https://github.com/rust-windowing/winit/blob/v0.30.13/FEATURES.md
- Platform impls @v0.30.13: `src/platform_impl/{windows/window.rs, macos/window_delegate.rs, linux/x11/window.rs, linux/wayland/window/mod.rs, linux/wayland/seat/pointer/relative_pointer.rs, web/window.rs, web/web_sys/event.rs}`
- https://github.com/rust-windowing/winit/blob/master/winit/src/changelog/v0.31.md
- https://github.com/rust-windowing/winit/blob/master/winit-web/src/lock.rs
- https://github.com/rust-windowing/winit/blob/master/winit-web/src/web_sys/event.rs
- Issues:
  - https://github.com/rust-windowing/winit/issues/1194
  - https://github.com/rust-windowing/winit/pull/3609
  - https://github.com/rust-windowing/winit/issues/3689
  - https://github.com/rust-windowing/winit/issues/4581
  - https://github.com/rust-windowing/winit/issues/4473
  - https://github.com/rust-windowing/winit/issues/4737
- Milestones: https://api.github.com/repos/rust-windowing/winit/milestones

**Browsers**: https://github.com/gpuweb/gpuweb/wiki/Implementation-Status (last edited 2026-10-02)

**Bevy**
- https://github.com/bevyengine/bevy/blob/v0.20.1/docs/cargo_features.md
- https://github.com/bevyengine/bevy/blob/v0.20.1/Cargo.toml
- https://github.com/bevyengine/bevy/blob/v0.20.1/crates/bevy_render/src/pipelined_rendering.rs
- Outdated size guidance: https://bevy-cheatbook.github.io/platforms/wasm/size-opt.html

**SDL**
- https://github.com/libsdl-org/SDL/pull/14741
- https://github.com/libsdl-org/SDL/issues/10146
- https://github.com/vhspace/sdl3-rs
- https://codeberg.org/maia/sdl3-sys-rs
- https://github.com/vhspace/sdl3-rs/issues/450

**Other engines**
- https://github.com/not-fl3/miniquad (README)
- https://github.com/FyroxEngine/Fyrox (README)
- https://github.com/kvark/blade/blob/main/blade-graphics/README.md

**GUI**
- https://github.com/emilk/egui/blob/0.37.0/crates/eframe/Cargo.toml
- https://github.com/emilk/egui/issues/5889
- https://github.com/emilk/egui/blob/0.37.0/crates/egui-wgpu/src/renderer.rs
- https://github.com/AccessKit/accesskit (README)
- https://github.com/iced-rs/iced (README)
- https://github.com/iced-rs/iced/issues/552
- https://github.com/slint-ui/slint/blob/master/LICENSE.md
- https://github.com/slint-ui/slint/blob/master/LICENSES/LicenseRef-Slint-Royalty-free-2.0.md
- https://docs.slint.dev/latest/docs/slint/guide/platforms/web/
- https://github.com/DioxusLabs/dioxus (README)
- https://github.com/linebender/xilem (README)
- npm registry: https://registry.npmjs.org/{uplot,echarts,plotly.js-dist-min,chart.js}

**Rust**
- https://static.rust-lang.org/dist/channel-rust-stable.toml
- https://doc.rust-lang.org/stable/rustc/platform-support.html
- https://doc.rust-lang.org/stable/cargo/reference/unstable.html#profile-trim-paths-option
- https://doc.rust-lang.org/stable/rustc/command-line-arguments.html#option-remap-path-scope

**Packaging**
- https://axodotdev.github.io/cargo-dist/book/installers/index.html
- https://github.com/crabnebula-dev/cargo-packager
- https://docs.rs/cargo-packager/latest/cargo_packager/config/struct.MacOsConfig.html
- https://docs.rs/cargo-packager/latest/cargo_packager/config/struct.WindowsConfig.html
- https://github.com/burtonageo/cargo-bundle (README)

**CI and signing**
- https://docs.github.com/en/actions/reference/runners/github-hosted-runners
- https://learn.microsoft.com/azure/artifact-signing/quickstart
- https://www.devclass.com/security/2026/01/14/code-signing-windows-apps-may-be-easier-and-more-secure-with-new-azure-artifact-service/4079554 (secondary)
- https://learn.microsoft.com/en-us/answers/questions/5810735/cant-create-a-new-trusted-signing-individual-ident (secondary)
- https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution (JS-rendered; content not readable → UNVERIFIED)

**Profiling**
- https://github.com/nagisa/rust_tracy_client (README compat table)
- https://github.com/gungraun/gungraun (README)
