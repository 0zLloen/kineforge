# WEB_PLATFORM_RESEARCH — the browser as a first-class KineForge target

Research dates 2026-11-13/14. Primary sources: W3C/WHATWG specs (WebGPU ED
2026-11-10, Pointer Lock 2.0 ED, Pointer Events ED, HR-Time ED 2026-11-06,
WHATWG File System LS 2026-10-19, WHATWG HID LS 2026-11-04), MDN
browser-compat-data @ `69fcf3608174` (2026-11-13), the gpuweb
Implementation-Status wiki (edited 2026-10-02), and browser **source code**
(Chromium `main` ≈ `a0facb83b514`, Firefox ≈ `6beeccd5eef3`, WebKit ≈
`cebeae1c387e`). Local runtime checks on Chromium 156 and Firefox 159 (Linux
X11, NVIDIA + AMD). **UNVERIFIED** = not confirmed. Full notes:
[`research/notes/web_platform.md`](../research/notes/web_platform.md) and
[`research/notes/_browser.md`](../research/notes/_browser.md).

## 0. Conclusions that shape the stack

1. **One shared Rust codebase including the wgpu renderer compiles to the browser** (strategy A, §9). The simulation/scoring/telemetry core stays free of wgpu, winit and web-sys, so other strategies remain cheap.
2. **A WebGL2 fallback is mandatory.** Firefox has no WebGPU on Linux or Intel Macs; Chrome on Linux enables it only for Intel Gen12+ and NVIDIA-on-Wayland. Field data: working WebGPU adapters on only 19.4 % of Linux visitors (web3dsurvey). All shaders target the WebGL2 subset (no compute, no storage buffers).
3. **Browser input is never device counts.** At best it is the OS's raw (pre-acceleration) delta relayed by the browser — and only on Windows/macOS/ChromeOS. **On Linux every browser rejects `unadjustedMovement`**, so browser movement there is always OS-accelerated unless the user configures libinput's flat profile (which a page cannot verify).
4. **Serve cross-origin-isolated (COOP + COEP) from day one.** It is the only way to get usable event timing: `performance.now()` measured locally at 0.1 ms → 0.005 ms (Chromium) and 1 ms → 0.02 ms (Firefox). It requires a host that can set headers (Cloudflare Pages / Netlify — **not** GitHub Pages without a service-worker workaround).
5. **The web build is display-paced.** No present modes (FIFO/rAF only); Chrome/Firefox run rAF at the display rate; **Safari has defaulted to ≈60 Hz** rAF (behind a default-on flag; whether Safari 26/27 changed this is unverified); hidden tabs get no rAF.
6. **Single-threaded on stable Rust.** Wasm threads still need nightly + `build-std` + extra linker args. Telemetry writes go to OPFS from a plain JS dedicated Worker via `postMessage` (transferable buffers) — no COI dependency for correctness.
7. **Local-first storage needs `persist()` + export.** Safari deletes script-writable storage after 7 days of use without interaction (unless installed to Dock/Home Screen).

## 1. GPU

### 1.1 WebGPU availability (stable channels; gpuweb wiki 2026-10-02 + BCD; adversarially re-verified)

| Browser | Windows | macOS | Linux | ChromeOS | Android | iOS/iPadOS |
|---|---|---|---|---|---|---|
| Chrome/Edge (Dawn) | ✅ 113 x86/x64; **ARM64: flag** | ✅ 113 | ✅ **Intel Gen12+ (144)**, **NVIDIA ≥535.183.01 on Wayland (147)**; others: flag | ✅ 113 | ✅ 121 (ARM/Qualcomm/Intel, Android 12+); Imagination Android 16+ (139); others TBD | n/a |
| Firefox (wgpu) | ✅ 141 | ✅ Apple Silicon: 145 on macOS 26+, 147 all versions; **Intel Macs: Nightly only** | ❌ **Nightly only** (not in 159) | — | ❌ flag only | n/a |
| Safari | — | ✅ 26 | — | — | — | ✅ 26 (also visionOS) |

Firefox stable has no WebGPU in service workers. WebGPU is `SecureContext`-only (HTTPS or `localhost`/`127.0.0.1`).

**Local check (this machine, X11 + NVIDIA):** headless Chromium 156 exposes `navigator.gpu` but `requestAdapter()` returns `null` by default; Firefox 159 has no `navigator.gpu` but **returns a `GPUCanvasContext` from `OffscreenCanvas.getContext('webgpu')`** anyway. ⇒ Detect WebGPU only by successfully obtaining an adapter. With `--enable-unsafe-webgpu`, the *first* `requestAdapter()` returned `null` in 3/3 runs and later calls succeeded (headless/flagged only; retry-once-before-fallback is cheap insurance — UNVERIFIED in default headed configurations).

### 1.2 WebGPU behaviour relevant to KineForge

- **Presentation**: no present modes, no frame-latency control; the drawing buffer is presented in the event loop's "update the rendering" step. Canvas context options (incl. `desynchronized`) are ignored for `webgpu`. wgpu's web backend reports `[Fifo]` only and panics on other modes (verified in v30.0.2 source).
- **Timestamp queries**: spec coarsens queue timestamps with cross-origin isolation *never applied* → 100 µs; Chrome documents 100 µs quantisation (lifted only by a developer flag). WebGL2 via wgpu: no GPU timestamps at all. ⇒ Web frame timing uses CPU-side `performance.now()` around submit + rAF timestamps.
- **Device loss**: `GPUDevice.lost` resolves (`unknown`/`destroyed`); objects never become valid again; recovery = new adapter + device + recreate every resource and reconfigure the canvas. **KineForge policy**: abort the current trial, log `context_lost`, rebuild the renderer from CPU-side state, continue with the next trial. wgpu v30's web device-lost API surface: UNVERIFIED (to check in the prototype).
- **Canvas format**: always use `getPreferredCanvasFormat()` (e.g. `rgba8unorm` unsupported for canvases on macOS Chrome).
- **Workers/OffscreenCanvas**: available, but input events arrive only on the main thread, so rendering in a worker adds a `postMessage` hop per input sample (or needs SharedArrayBuffer ⇒ COI). **v1: render on the main thread.**
- **Default limits** (core): `maxTextureDimension2D` 8192, `maxBufferSize` 256 MiB, 8 storage buffers/stage. Compatibility mode (`featureLevel: "compatibility"`, Chrome 146 on GLES 3.1) has 0 storage buffers in the vertex stage. Field data: `timestamp-query` on 98.8 % of WebGPU adapters.

### 1.3 WebGL2 through wgpu

- One binary can carry both: with `webgpu` + `webgl` features, `wgpu::util::new_instance_with_webgpu_detection` picks WebGPU if an adapter is obtainable, else WebGL2 (verified in v30.0.2 `wgpu/src/util/init.rs`).
- WebGL2 path (`Limits::downlevel_webgl2_defaults()`): **no storage buffers/textures in any stage, no compute**, 11 uniform buffers/stage, 15 inter-stage variables; flags off: `BASE_VERTEX`, `INDEPENDENT_BLEND`, cube arrays, `noperspective`. Request `adapter.limits()` for real texture limits.
- Shaders: WGSL → naga → GLSL ES 3.00. **Design rule: write all shaders for the WebGL2 subset** (vertex + uniform buffers; instancing via vertex buffers).
- Size: `webgl` links wgpu-core + hal + naga. Measured deployed artifacts (§3.3): wgpu examples code section 2.23 MB (WebGPU-only) vs 8.33 MB (WebGL2-capable); egui measured +2.4 MiB uncompressed for its demo.

## 2. Input in the browser

### 2.1 What a browser can and cannot measure

**Can:**
1. **OS-raw deltas behind Pointer Lock with `unadjustedMovement: true`** — Chrome ≥88 on Windows (`RAWMOUSE.lLastX/Y` per `WM_INPUT`), macOS (`kCGEventUnacceleratedPointerMovementX/Y`), ChromeOS; Firefox ≥152 on Windows/macOS; Safari ≥18.4 on macOS. Integers in Chrome/Firefox, `double` in Safari. Not proven device counts.
2. **Accelerated pointer deltas everywhere else** — including **every browser on Linux X11 and Wayland**, which reject `unadjustedMovement` with `NotSupportedError` (Chromium: no `RequestUnadjustedMovement` override for Linux; Firefox: GTK has no `SupportsUnadjustedMovement` override; WebKitGTK/WPE: `HAVE_MOUSE_UNACCELERATED_MOVEMENT` is Mac-only).
3. **Sub-frame event history** — `getCoalescedEvents()` (Chrome, Firefox, Safari ≥18.2 — but WebKit on macOS lists only the event itself) and `pointerrawupdate` (Chrome; Firefox ≥148; not Safari), both secure-context only. Movement is summed when coalescing, so per-OS-event deltas can be rebuilt (Chrome on Windows: one per `WM_INPUT`).
4. **Event timestamps** on the page's monotonic clock, coarsened (§2.4).

**Cannot:**
- **Raw HID mouse reports.** Chromium always protects input/output reports of Generic Desktop **Pointer (0x01) and Mouse (0x02)** collections (feature reports allowed) and keyboards; WebUSB protects HID class 0x03; the Gamepad API never lists mice. Only vendor-defined collections (e.g. the ODD config channel, usage page 0xFFFF) are reachable via WebHID — Chromium only, user gesture + chooser, secure context.
- **Which mouse moved** (all mice merge into one pointer), **device CPI**, **true report rate**, **HID report timestamps**, **device-side processing**.
- **Unaccelerated motion on Linux** (see above).

**Rule:** browser data is recorded as `browser_locked_unadjusted` (only when the request with `unadjustedMovement: true` actually resolved), `browser_locked`, or `browser_unlocked` — **never** `device_report` or `os_raw_counts` (DATA_SPECIFICATION.md §3.1). KineForge must never silently fall back from unadjusted to adjusted lock (Open Directional DPI's calibrator currently does — OPEN_DIRECTIONAL_DPI_AUDIT.md §5); a fallback is allowed only if it is recorded and shown to the user.

### 2.2 `unadjustedMovement` support (source-verified)

| | Windows | macOS | Linux X11 | Linux Wayland | ChromeOS | Android |
|---|---|---|---|---|---|---|
| Chrome ≥ 88 | ✔ WM_INPUT raw | ✔ CGEvent unaccelerated | ✘ | ✘ | ✔ Ozone evdev | ✔ 144 (mechanism UNVERIFIED) |
| Firefox ≥ 152 | ✔ WM_INPUT raw | ✔ CGEvent unaccelerated | ✘ | ✘ | n/a | ✘ |
| Safari ≥ 18.4 | n/a | ✔ (double) | ✘ (WebKitGTK) | ✘ | n/a | iPadOS UNVERIFIED |

Feature detection requires an actual request (catch `NotSupportedError`).

### 2.3 `movementX/Y` units

| Browser/OS/mode | Units | Type | Notes |
|---|---|---|---|
| Chrome Windows, unadjusted | OS raw units (`lLastX`; ≈ counts for relative HID mice) | integer | not DPR/zoom scaled |
| Chrome Windows/Linux, adjusted | DIP screen deltas | integer (truncated positions) | DPR scaling removed in Chrome (2022, commit `66ad18c093`); hidden cursor recentred within 15 % of the border |
| Chrome Linux Wayland | **accelerated** `dx` (`dx_unaccel` decoded then discarded) | integer | receipt timestamps |
| Firefox X11/Windows adjusted | CSS px (DPR **and** page zoom applied) | integer | positions rounded before differencing |
| Firefox Wayland | `int32(dx × scale)` per event, **no remainder carry** | integer | slow motion biased toward zero; `utime_lo` (µs) apparently fed to a ms converter — timestamps suspect |
| Firefox macOS | `int32(unaccelerated)` or truncated `deltaX` | integer | |
| Safari macOS | points or unaccelerated units | `double` | |

KineForge must record browser, OS, DPR, page zoom and lock mode with every session and never mix them.

### 2.4 Timestamps

| Browser | `event.timeStamp` / `performance.now()` resolution | What instant the event timestamp represents |
|---|---|---|
| Chrome | **100 µs**, **5 µs with COI**, with jitter (measured locally: 0.1 ms / 0.005 ms) | Windows: QPC at **browser UI-thread receipt** (GetMessageTime ignored); X11: X server time (ms); Wayland: receipt time (relative-pointer µs `utime` unused); macOS: `NSEvent.timestamp` |
| Firefox | **1 ms with jitter**; **20 µs with COI** (measured locally: 1 ms / 0.02 ms) | Windows: `GetMessageTime()` (ms); X11/Wayland: X/GDK ms; macOS: `NSEvent.timestamp` |
| Safari | **1 ms** non-isolated; **20 µs with COI** (WebKit `Performance.cpp` `highTimePrecision()`, adversarial verification S3) — confirm on hardware | platform event time (`NSEvent.timestamp`) |

Implications: without COI, inter-event intervals at ≥1 kHz are meaningless in Firefox/Safari. Even with COI, Chrome-on-Windows timestamps include browser UI-thread queueing. Timestamps are recorded with their clock domain and declared resolution; per-event latency claims from browser data are never made.

### 2.5 Delivery

- Chrome dispatches continuous events (`pointermove`, `mousemove`, wheel) **rAF-aligned** and coalesced; discrete events (down/up, keys) immediately. `pointerrawupdate` bypasses rAF alignment (dispatched ASAP; may self-coalesce under load).
- KineForge web capture: listen to `pointerrawupdate` where available (Chrome, Firefox ≥148), expand `getCoalescedEvents()`, otherwise `pointermove` + coalesced list; record each sub-event with its own timestamp and a `coalesced` flag.

### 2.6 Lock lifecycle

- Requires **transient user activation** (Chrome/WebKit also allow re-lock if the page itself released the lock). Request pointer lock **before** fullscreen in the same gesture (fullscreen consumes activation).
- **Chrome re-lock cooldown: 1250 ms after a user Esc** (`SecurityError`); **rate limit: >10 requests in 2 s** → `NotAllowedError`. No equivalent found in Firefox/WebKit (UNVERIFIED).
- Focus loss or hidden document exits the lock. Esc always exits unless fullscreen + Keyboard Lock (Chromium only; then hold Esc 1.5 s).
- KineForge logs `pointerlockchange`, `pointerlockerror`, `visibilitychange`, `blur` as annotations, pauses the trial, marks the gap, never interpolates across it.

### 2.7 Calibration validity by input level

Directional-gain fitting and firmware-profile export follow the normative rule in EXPERIMENT_PLAN E8: allowed only from `device_report`, `os_raw_counts`, `os_unaccelerated` with acceleration verified absent, or `browser_locked_unadjusted` — never `os_absolute_pointer` or `browser_locked`. With accelerated input (`browser_locked` — every Linux browser), gain is speed-dependent and flick speed varies with distance and direction, so acceleration would be fitted as directional error; such data is shown as indicative only and `kf-calib` refuses to export a profile. Constant unit scaling (DIP vs DPR, unverified macOS units) is acceptable only with mean-preserving normalisation and the scale recorded. EXPERIMENT_PLAN E8 step 0 measures the error directly on this machine (`hidraw2` vs browser stream, same flicks).

### 2.8 Capture rules (double-counting and fallbacks)

- Listen to exactly **one** motion event type per session: `pointerrawupdate` where it delivers non-zero movement (Chrome; Firefox ≥ 148 — 140–147 reported zero movement), otherwise `pointermove`. Record only **leaf** events (coalesced-list entries, or the event itself when its list is empty or contains only itself) — a parent's `movementX` is the sum of its children.
- Every pointer-lock acquisition opens a **new source** declaring the level that request actually obtained; there is no silent unadjusted → adjusted fallback. The simulation may switch to the new source only if level, units and scale are identical; otherwise the session ends (`input_level_changed`) and a new one starts (DATA_SPECIFICATION.md §2.3).

### 2.9 Degradation matrix (requirement 23: usable with graceful degradation)

| Capability failure | Behaviour | User message | Telemetry |
|---|---|---|---|
| `unadjustedMovement` rejected | aiming tasks run on accelerated lock; calibration export disabled | "Your browser/OS applies pointer acceleration here; results are labelled and calibration export is disabled" | source `browser_locked` |
| Pointer Lock unavailable or repeatedly refused (iframe sandbox, cooldown, touch-only device) | aiming tasks disabled; import, analytics, replay and the sensitivity converter still work | explanation + retry button (respects Chrome's 1250 ms cooldown) | no aiming session created |
| Neither WebGPU nor WebGL2 | static HTML message in `index.html` (egui cannot draw); non-3D tools offered if they don't need a canvas | "No usable GPU API" with browser/flag hints | none |
| No OPFS sync access handle | practice without recording | "Recording unavailable in this browser/mode (e.g. private window)" | no session |
| `QuotaExceededError` / storage error mid-session | session ends at a chunk boundary (`end_reason = storage_error`), export prompted | "Storage full — export now" | `storage_error` gap |
| Sessions evicted from OPFS (Safari 7-day rule, quota pressure) | on startup, missing and unexported sessions are reported | list of affected sessions | — |
| Not cross-origin isolated (host without headers) | runs; timestamps flagged coarse | "Timing precision reduced (100 µs–1 ms)" | clock domain `coi: false` |
| Safari / low-rAF browser | runs; achieved rAF shown | "Display updates limited to N Hz in this browser" | `rAF_hz_measured` in `session_end` |

## 3. Rust → wasm runtime and delivery

### 3.1 Toolchain status

| Item | Status |
|---|---|
| rustwasm org | sunset (archived Sept 2025); **wasm-bindgen moved to the `wasm-bindgen` org** (0.2.130, 2026-10-30), wasm-pack transferred and alive (0.15.1) |
| Trunk | 0.22.1 (2026-10-26); default `data-bindgen-target` is `no-modules` — set `web` |
| binaryen (`wasm-opt`) | version_133; the `wasm-opt` crate is stale — use release binaries |
| `console_error_panic_hook` | archived; it is ≈10 lines — inline it |
| `wasm32-unknown-unknown` defaults | multivalue, mutable-globals, reference-types, sign-ext, nontrapping-fptoint, bulk-memory (since Rust 1.87/LLVM 20) — all supported by every WebGPU-capable browser; `simd128` opt-in |
| Panic strategy | `panic=abort` (unwind needs nightly + build-std) |
| CLI pinning | wasm-bindgen CLI must equal the library version exactly; derive it from `Cargo.lock` (as wgpu's CI does) |

**Recommended build (minimal):** `cargo build --release --target wasm32-unknown-unknown` → `wasm-bindgen --target web` (version from `Cargo.lock`) → optional `wasm-opt -Oz` (measure) → static `index.html` loading the ES module. Trunk is an optional convenience. No Vite unless a TS/DOM UI appears.

### 3.2 Threads

SharedArrayBuffer requires COI. Rust wasm threads require **nightly**, `+atomics,+bulk-memory`, `-Zbuild-std=panic_abort,std`, and (since rust#147225, Oct 2025, and rust#156174, May 2026) explicit `--shared-memory --import-memory --max-memory --export=__wasm_init_tls …__heap_base` linker args. **v1 is single-threaded on stable**; off-main-thread work uses plain dedicated Workers with `postMessage`.

### 3.3 Size

| Artifact (deployed, measured) | Raw | gzip -9 | Code section |
|---|---|---|---|
| egui.rs demo (stripped) | 11.5 MB | 4.52 MB | 9.57 MB |
| wgpu examples, WebGPU-only (unstripped, ~30 examples + assets) | 8.10 MB | 2.82 MB | 2.23 MB |
| wgpu examples, WebGL2-capable | 18.0 MB | 5.41 MB | 8.33 MB |

These are upper bounds for unrelated apps; **KineForge's own size is a measurement to make in the first web milestone** (target in PERFORMANCE_PLAN W1). Strip names/DWARF from release builds; keep `--split-debug-info` artifacts for debugging. Decide single-binary (WebGPU + WebGL2) vs two artifacts after measuring.

### 3.4 Delivery details

- `WebAssembly.instantiateStreaming` requires `Content-Type: application/wasm` (GitHub Pages, Cloudflare Pages and Netlify all serve it correctly — observed for GitHub Pages).
- CSP: `script-src 'self' 'wasm-unsafe-eval'` allows wasm without JS `eval` (Chrome 97, Firefox 102, Safari 16). `connect-src 'self'` turns "no telemetry upload" into a browser-enforced guarantee. Trunk's inline loader vs strict CSP: UNVERIFIED — use an external module file.
- Debugging: DWARF via wasm-bindgen `--keep-debug`/`--split-debug-info` and Chrome's DWARF extension (Rust stepping support UNVERIFIED).
- Memory: wasm32 up to 4 GB in Chrome (2020); other browsers' per-tab ceilings UNVERIFIED. KineForge streams the log to OPFS and pre-reserves buffers to avoid `memory.grow` mid-trial. memory64 (Chrome 133, Firefox 134, Safari not yet) and `wasm64-unknown-unknown` (Tier 3): **not used**.

## 4. Timing and scheduling

- rAF follows display vsync in Chrome and Firefox (reports of 240/480/540 fps on matching displays — secondary measurements). **Safari: "Prefer Page Rendering Updates near 60fps" is on by default** (≈60 Hz; 120 Hz ProMotion only with the flag off), ≈30 fps in Low Power Mode and in iframes. Current Safari 26/27 default: UNVERIFIED → measure. Headless browsers report synthetic ≈60 Hz (useless for this).
- Hidden tabs: no rAF; timers throttled to 1 s (Chrome intensive throttling: 1/min after 5 min hidden; Firefox Android 15 min). ⇒ On `visibilitychange: hidden` or lock loss, pause and mark the gap; never sample input with timers.
- Main-thread jank sources to avoid: per-frame JS allocations across the boundary, `memory.grow`, first-use shader/pipeline compilation (create all pipelines at load).
- `scheduler.postTask`: no Safari — not needed.
- KineForge displays the **achieved** rAF rate and warns when it is below the display's refresh rate.

## 5. Storage, offline, privacy

| | Chrome/Edge | Firefox | Safari 17+ |
|---|---|---|---|
| Per-origin quota | ≈60 % of disk | best-effort: min(10 % of disk, 10 GiB group); persistent: 50 % | ≈60 % (browser app) |
| `persist()` | heuristic, no prompt | **user prompt** | heuristic (e.g. Home Screen/Dock app) |
| Eviction | LRU under pressure (non-persistent) | same | LRU **+ ITP: all script-writable storage deleted after 7 days of Safari use without interaction**, unless installed (Dock/Home Screen) |

- **OPFS `FileSystemSyncAccessHandle`**: dedicated Workers only, secure context, exclusive lock per file; synchronous `flush/getSize/truncate/close` from Chrome 108, Firefox 111, Safari 16.4 (Baseline widely available since Sept 2025). `flush()` "attempts" to persist; `close()` does not guarantee persistence without `flush()` first. Verified locally (write/flush/getSize) in Chromium 156 and Firefox 159 workers. Whether OPFS is covered by Safari's 7-day rule: UNVERIFIED — assume yes.
- **Export**: `showSaveFilePicker` is Chromium-only; elsewhere a `Blob`/OPFS `File` with `<a download>`. Sessions are tens of MB; streaming export libraries are unnecessary.
- **IndexedDB**: only for small settings/index records (if at all). **SQLite-wasm**: not in v1 (the `opfs-sahpool` VFS needs no COI but is Worker-only and allows only limited concurrency; a second tab cannot install it).
- **PWA**: Chromium installable with a manifest (service worker not required); Safari 17+ "Add to Dock"; iOS "Add to Home Screen"; Firefox desktop has no manifest install. Installing matters on Safari (escapes the 7-day counter). Service worker: precache hashed assets, network-first `index.html`, updates applied **only between sessions**.
- **Secure-context requirements**: WebGPU, OPFS, service workers, `pointerrawupdate`, `getCoalescedEvents`, WebHID all need HTTPS or localhost; SharedArrayBuffer additionally needs COI. Development uses a local HTTP server, never `file://`.
- **Privacy defaults**: no network calls after load (CSP-enforced), no analytics, explicit export; adapter vendor/architecture recorded only in the local session.

## 6. Static hosting

| Host | Custom headers (COOP/COEP/CSP) | Verdict |
|---|---|---|
| **Cloudflare Pages** | ✅ `_headers` (≤100 rules; not applied to Functions) | canonical deploy candidate |
| **Netlify** | ✅ `_headers` / `netlify.toml` | canonical deploy candidate |
| GitHub Pages | ❌ (long-standing request); gzip only; `max-age=600` | mirror only, with COOP/COEP injected by our own service worker (coi-serviceworker technique: first-load reload, conflicts with a separate PWA worker, hard reload loses COI) |

Ship as a **top-level page**, not inside third-party iframes (COI inheritance, Safari 30 fps iframe cap).

```
/*
  Cross-Origin-Opener-Policy: same-origin
  Cross-Origin-Embedder-Policy: require-corp
  Content-Security-Policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; connect-src 'self'; img-src 'self' data: blob:; style-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'
  X-Content-Type-Options: nosniff
/assets/*
  Cache-Control: public, max-age=31536000, immutable
```
(`require-corp` rather than `credentialless`: Safari lacks `credentialless`; free for a same-origin app.)

## 7. Accessibility

Canvas UIs (egui on web) are invisible to screen readers: egui offers only an experimental speech-synthesis reader on the web, and AccessKit's web adapter is "planned". The aiming task itself is inherently visual/motor, but menus, settings and results need not be. **v1 baseline**: full keyboard operability of menus, `prefers-reduced-motion`/`prefers-contrast`, colour-blind-safe target palettes, scalable UI, results exportable as CSV/Parquet. **Upgrade path**: a DOM settings/results layer around the canvas (strategy C) if web accessibility becomes a requirement — a decision requested in STACK_DECISION.md.

## 8. Feature matrix — native vs web

Status vocabulary: **supported**, **supported with limitation**, **requires permission**, **native only**, **unknown**. "Native" columns describe the platform capability under the proposed stack (nothing is implemented yet; each row is verified by the listed experiment).

| Feature | Windows native | Linux native | macOS native | Browser |
|---|---|---|---|---|
| Full 3D aim training, configurable scenarios | supported | supported | supported | supported with limitation — WebGPU or WebGL2 fallback; display-paced (FIFO/rAF); Safari rAF rate unknown on current versions (likely ≈60 Hz default) |
| Sensitivity, FOV, directional-gain simulation | supported | supported | supported | supported (same Rust core; designed to be bit-identical — verified by E10) |
| Scoring and analytics | supported | supported | supported | supported |
| Telemetry record / replay / export / import | supported | supported | supported | supported with limitation — OPFS quota/eviction (Safari 7-day ITP unless installed), `persist()` prompt on Firefox, export via Blob except Chromium's save dialog |
| Local calibration and experimental tools | supported | supported | supported | supported with limitation — fitting/firmware export only from `browser_locked_unadjusted` (Windows/macOS/ChromeOS); accelerated streams (all Linux browsers) are indicative only and export is refused (§2.7) |
| Device-level raw counts | supported with limitation — Raw Input gives OS raw counts per report; HID reports of the mouse collection are opened exclusively by the OS | requires permission — evdev (`input` group/udev) gives kernel counts; hidraw gives HID reports (udev rule) | requires permission — IOHID with Input Monitoring (TCC) | **native only** — browsers never expose HID mouse reports |
| Multi-mouse (per-device) access | supported (Raw Input device handles) | requires permission (per-device evdev nodes) | requires permission (IOHID per device) | **native only** — all mice merge into one pointer |
| Firmware interfaces (ODD vendor HID channel) | supported (hidapi to vendor collection) | requires permission (hidraw node access via udev; already granted for the Glorious receiver on this machine) | unknown — vendor-collection access without Input Monitoring to be tested | requires permission — WebHID, **Chromium only**, user gesture + chooser; unavailable in Firefox/Safari |
| System-wide input capture / background logging | supported with limitation — `RIDEV_INPUTSINK`; background raw-input delivery behaviour on Windows 11 to be measured | requires permission (evdev) | requires permission (Input Monitoring) | **native only** — impossible by design |
| Unaccelerated motion | supported (Raw Input) | supported (evdev; XI2 raw; Wayland relative-pointer unaccelerated) | supported with limitation — via IOHID/GameController paths (see TECH_STACK_RESEARCH §3) | supported with limitation — Chrome/Firefox on Windows & macOS, Safari macOS, ChromeOS; **not on Linux** |
| High-refresh rendering (>144 Hz) | supported (Immediate/Mailbox/Fifo) | supported (Immediate; Mailbox on NVIDIA Vulkan/Wayland) | supported with limitation (Metal; ProMotion ≤120 Hz on laptops) | supported with limitation — display rate in Chrome/Firefox; Safari unknown on current versions (likely ≈60 Hz default) |
| Input-timing precision | supported with limitation — Raw Input has **no per-event OS timestamp**; QPC at capture-thread drain (`app_dequeue`); GameInput µs base unverified (X-W4) | supported with limitation — default X11 raw path: X-server ms (`os_dispatch`); kernel µs via evdev requires permission; Wayland `utime` µs (ms on wlroots) | supported — IOHID AbsoluteTime per report (with Input Monitoring); otherwise NSEvent timestamps | supported with limitation — with COI: 5 µs (Chrome), 20 µs (Firefox, Safari); without: 100 µs (Chrome), 1 ms (Firefox, Safari); what the stamp means varies by browser × OS |
| Input-latency benchmarking | supported with limitation — host timing in-app; photon side needs an external photodiode (wgpu has no present-time API) | same | same | supported with limitation — photodiode rig works; host-side numbers are browser-processing only and are labelled so |

Experiments that resolve the "unknown"/limited cells (also in EXPERIMENT_PLAN.md): browser X-B1–X-B11 (unadjusted support, ground truth vs browser on Linux via hidraw, DPR 1.25 and page zoom, timestamp domains with/without COI, Windows 1/4/8 kHz report-rate visibility, re-lock cooldown, absolute pointers under RDP/VM, Firefox Wayland truncation and timestamp bug, macOS units vs IOHID, rAF vs `pointerrawupdate` latency), headed rAF at 144/165/240/360 Hz, macOS vendor-HID access without TCC, Windows 11 background raw input.

## 9. Architecture strategies (Phase 0.9 A–D)

| | (A) One Rust codebase incl. wgpu render + egui UI | (B) Shared Rust core + separate native/web graphics adapters | (C) DOM/TS UI around a wasm canvas | (D) TS/WebGPU renderer, Rust core only |
|---|---|---|---|---|
| Portability | ✅ same render code on DX12/Vulkan/Metal/WebGPU/WebGL2 | ✅ but two renderers to keep identical | render ✅ (A's), but native still needs a UI ⇒ two UIs | web-only renderer; native needs another ⇒ two renderers |
| Web performance | display/rAF-bound like every option | same ceiling | same + DOM layout on main thread (keep idle in trials) | same GPU ceiling |
| Bundle | largest (wgpu + egui, + naga for WebGL2) | can be smaller | A minus egui + small TS | smallest wasm, renderer in JS |
| Testability | native unit tests cover sim + render logic; one web smoke suite | two render test matrices | + TS/Playwright | two renderers + parity tests |
| Maintainability | one language, one shader language (WGSL) | drift risk | two UI stacks + TS toolchain | two renderers, two shader dialects |
| Web accessibility | ✗ | ✗ | ✅ | ✅ |

**Decision input: (A), with the core kept engine-free so (C) can be added for web accessibility without touching the simulation.**

### 9.1 Crate boundaries for web vs native

| Must compile for both native and wasm32 | Platform adapters (trait + per-target impl) | Native-only |
|---|---|---|
| `kf-core`, `kf-dirgain` (`no_std`), `kf-sim`, `kf-telemetry` (log format, readers, validator), `kf-analytics`, `kf-calib`, `kf-render` (wgpu), the trainer app shell | `kf-input` (Raw Input / evdev / XI2 / Wayland / macOS / Pointer Lock), `kf-store` (fs vs OPFS-worker) | `kf-cli`, HID/hidraw capture, evdev, system-wide logging, Parquet zstd export (web uses pure-Rust codecs), the C-oracle differential fuzzer |

Boundaries are enforced by **target-specific dependency tables**, not opt-in features, plus CI `cargo check --target wasm32-unknown-unknown` for every portable crate (ARCHITECTURE.md §2.1).

### 9.2 Minimum useful vertical slices (sharing one core)

- **Browser slice**: static page → capability report (WebGPU/WebGL2, unadjusted lock result, COI, OPFS) → one seeded static-flick scenario → Pointer Lock capture with provenance → scoring → session log to OPFS → export `.kfz` → re-import and replay with identical hashes.
- **Native slice (Linux first, then Windows)**: same scenario and core; winit window + wgpu; native raw input (evdev or XI2 on Linux, Raw Input on Windows) with OS timestamps; same log format; `kf-cli` validates and replays the session from either slice with identical simulation hashes.

### 9.3 Browser CI

- wgpu's own CI exercises only the **WebGL2** path in headless Chromium (SwiftShader).
- **WebGPU in headless Chromium works locally with SwiftShader** (`--headless=new --enable-unsafe-webgpu --use-webgpu-adapter=swiftshader --enable-features=Vulkan --use-angle=swiftshader` → fallback adapter, `timestamp-query` present). Running it on GitHub-hosted runners: likely but UNVERIFIED. Assert via buffer readback, not screenshots.
- Firefox WebGPU cannot be tested on Linux CI (not shipped); Safari needs macOS runners (headless status UNVERIFIED).
- Plan: native `cargo test` is the main gate; web CI = core tests under `wasm-bindgen-test` + one Playwright smoke test (load → adapter for WebGPU-SwiftShader and WebGL2 → render N frames → readback hash → OPFS write/read → export).

## 10. Open questions and how to verify them

| # | Question | Verification |
|---|---|---|
| 1 | Safari rAF default on ProMotion / external high-refresh displays; Safari WebGPU limits, timestamp-query, COI timer value | manual run on an Apple Silicon Mac |
| 2 | Headed rAF at 144/165/240/360 Hz per browser and OS, incl. mixed-refresh multi-monitor | rAF probe page on real displays (this machine: 165 Hz) |
| 3 | First-`requestAdapter()`-null in default headed configurations | probe on several machines; implement retry-once |
| 4 | KineForge's wasm size (gzip/brotli), single vs dual artifact | first web milestone measurement |
| 5 | WebGPU via SwiftShader on GitHub Actions | one CI job |
| 6 | Per-tab wasm memory ceilings (Firefox, Safari, iOS) | allocation probe |
| 7 | Trunk/strict-CSP compatibility; egui/winit `style-src` needs | build + CSP report-only |
| 8 | wgpu v30 web device-lost / WebGL context-loss behaviour | fault injection (`GPUDevice.destroy()`, `WEBGL_lose_context`) |
| 9 | Browser input anomalies (Chrome Wayland drops unaccelerated deltas; Firefox Wayland truncation and µs-as-ms timestamps; Firefox ignores absolute raw input; WebKit coalesced list) | browser experiments X-B1–X-B11 (EXPERIMENT_PLAN.md §P) |
| 10 | Whether Chrome's `pointerrawupdate`/coalesced lists stay complete at 4–8 kHz | Windows + 8 kHz mouse + native Raw Input logger in parallel |

## 11. Sources (accessed 2026-11-13/14)

Specifications and status pages:
- WebGPU spec (ED 2026-11-10): https://gpuweb.github.io/gpuweb/ · W3C TR (CRD 2026-09-15): https://www.w3.org/TR/webgpu/
- gpuweb Implementation Status (edited 2026-10-02): https://github.com/gpuweb/gpuweb/wiki/Implementation-Status
- MDN browser-compat-data (commit 69fcf36081): https://github.com/mdn/browser-compat-data (`api/GPU.json`, `api/Element.json`, `api/PointerEvent.json`, `api/HID.json`, `api/FileSystemSyncAccessHandle.json`, `http/headers/*`) · caniuse WebGPU: https://caniuse.com/webgpu
- Pointer Lock 2.0 ED: https://w3c.github.io/pointerlock/ · Pointer Events ED: https://w3c.github.io/pointerevents/ · HR-Time ED: https://w3c.github.io/hr-time/ · DOM: https://dom.spec.whatwg.org/
- WHATWG File System: https://fs.spec.whatwg.org/ · WHATWG HID: https://hid.spec.whatwg.org/ (blocklist https://github.com/whatwg/hid/blob/main/blocklist.txt) · WebUSB: https://wicg.github.io/webusb/ · Keyboard Lock: https://wicg.github.io/keyboard-lock/
- Open spec issues: https://github.com/w3c/pointerlock/issues/42, https://github.com/w3c/pointerlock/issues/98

Browser vendors:
- Chrome: https://developer.chrome.com/blog/new-in-webgpu-147-148 · https://developer.chrome.com/blog/new-in-webgpu-155-156 · https://developer.chrome.com/docs/web-platform/webgpu/developer-features (timestamp quantisation) · https://developer.chrome.com/blog/timer-throttling-in-chrome-88 · https://developer.chrome.com/blog/aligning-input-events
- Chromium source (main ≈ a0facb83b514): https://github.com/chromium/chromium (`content/browser/renderer_host/render_widget_host_view_event_handler.cc`, `ui/views/win/hwnd_message_handler.cc`, `third_party/blink/renderer/core/page/pointer_lock_controller.cc`, `third_party/blink/renderer/core/timing/time_clamper.h`, `services/device/public/cpp/hid/hid_report_utils.cc`)
- Firefox: https://www.firefox.com/en-US/firefox/159.0/releasenotes/ · https://bugzilla.mozilla.org/show_bug.cgi?id=2037802 (unadjustedMovement, Fx 152) · source https://github.com/mozilla-firefox/firefox (`dom/base/PointerLockManager.cpp`, `widget/gtk/nsWindowWayland.cpp`, `toolkit/components/resistfingerprinting/nsRFPService.cpp`)
- WebKit: https://github.com/WebKit/WebKit (`Source/WebCore/page/PointerLockController.cpp`, `Source/WebCore/page/Performance.cpp`) · https://bugs.webkit.org/show_bug.cgi?id=173434 (rAF 60 fps) · https://webkit.org/blog/14403/updates-to-storage-policy/ · https://webkit.org/blog/10218/full-third-party-cookie-blocking-and-more/

Platform documentation:
- MDN: https://developer.mozilla.org/en-US/docs/Web/API/Storage_API/Storage_quotas_and_eviction_criteria · https://developer.mozilla.org/en-US/docs/Web/API/Performance/now · https://developer.mozilla.org/en-US/docs/Web/API/FileSystemSyncAccessHandle · https://developer.mozilla.org/en-US/docs/WebAssembly/Reference/JavaScript_interface/instantiateStreaming_static · https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Guides/Making_PWAs_installable
- Rust wasm: https://doc.rust-lang.org/nightly/rustc/platform-support/wasm32-unknown-unknown.html · https://blog.rust-lang.org/inside-rust/2025/07/21/sunsetting-the-rustwasm-github-org/ · https://github.com/rust-lang/rust/pull/147225 · https://wasm-bindgen.github.io/wasm-bindgen/reference/cli.html · https://trunk-rs.github.io/trunk/guide/assets/
- wgpu: https://github.com/gfx-rs/wgpu (README, `wgpu/src/util/init.rs`, `wgpu-types/src/limits.rs`, `xtask/src/test_wasm.rs`)
- Hosting: https://developers.cloudflare.com/pages/configuration/headers/ · https://docs.netlify.com/manage/routing/headers/ · https://github.com/orgs/community/discussions/13309 (GitHub Pages headers) · https://github.com/gzuidhof/coi-serviceworker
- sqlite-wasm persistence: https://sqlite.org/wasm/doc/trunk/persistence.md
- Field data: https://web3dsurvey.com/webgpu
- Measured artifacts: https://www.egui.rs/egui_demo_app_bg.wasm · https://wgpu.rs/examples/webgpu_bg.wasm · https://wgpu.rs/examples/webgl2_bg.wasm
