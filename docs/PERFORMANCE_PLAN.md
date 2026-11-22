# PERFORMANCE_PLAN — KineForge

Status: research-phase plan, 2026-11-14 (revised after independent review).
**All numbers below are proposed targets, not measurements.** Each names the
procedure that tests it. Nothing is claimed about arbitrary hardware; targets
are per hardware class. Identifier conventions: **H1–H5** hardware classes,
**N1–N12** native targets, **W1–W10** web targets (experiment IDs elsewhere
use the `X-` prefix).

## 1. Hardware classes

| Class | Definition | Reference machine |
|---|---|---|
| **H1 Desktop, discrete GPU** | ≥6-core x86-64 (2020+), discrete GPU, 144–240 Hz display | **Available**: Ryzen 7 5700G, RTX 3070 Ti (NVIDIA 580.178.04), 2560×1440 @ 165 Hz, Linux Mint 22.3 X11 (Cinnamon). Windows install: unknown. Mouse: Glorious Series One PRO Wireless via a USB full-speed receiver (≤1 kHz) |
| **H2 Esports rig** | high-clock x86-64, high-end GPU, 360 Hz+ display, 4–8 kHz mouse | not available — needed for 360 Hz targets |
| **H3 Integrated GPU** | iGPU-only laptop/desktop (Intel Xe / AMD iGPU), 60–144 Hz | approximation: the 5700G's Vega iGPU (RADV) on H1 |
| **H4 Apple Silicon** | M-series Mac, ProMotion or external 144 Hz+ | not available — required for macOS/Safari targets |
| **H5 ARM64 Windows/Linux** | Snapdragon X / ARM64 Linux | not available — CI runners only |

Browser measurements are written "Web @ Hn (browser, OS/session)". A target is met only when measured on the named class with the procedure in §4.

## 2. Native targets

Frame budgets: 144 Hz = 6.94 ms, 240 Hz = 4.17 ms, 360 Hz = 2.78 ms, 500 Hz = 2.00 ms.

| ID | Metric | Target | Class | Procedure |
|---|---|---|---|---|
| N1 | CPU time per frame (input take + sim + record hand-off + render encode + UI), default scene | p99 ≤ 25 % of the frame budget at the display refresh; the egui pass's share reported separately | H1, H2, H3 | §4.1 |
| N2 | Frame-start interval stability, **Fifo only**, GPU not saturated | p99 interval ≤ 1.10 × refresh period; ≤ 0.1 % intervals > 1.5 × period over 5 min (CPU frame-start timestamps; cross-checked with PresentMon on Windows and the photodiode rig) | H1, H2 | §4.2 |
| N3 | Uncapped throughput, benchmark scene, minimal quality | ≥ 1000 fps at 1440p on H1 — a sanity check that the thin renderer is not the bottleneck at 360 Hz. **No engine comparison is planned**: the renderer decision rests on control, ordering, churn and size, not on speed | H1 | §4.2 |
| N4a | CPU cost per input event (decode → ring push → recorder encode → sim apply), single-threaded microbenchmark | median ≤ 2 µs/event on H1; regression-tracked by instruction count | H1 | §4.3 |
| N4b | Capture-to-sim handoff latency (receipt `t_ns` → applied), multithreaded, on hardware | p99 ≤ 1 ms outside frame waits; reported as a distribution | H1 | §4.3 |
| N5 | Steady-state heap allocations in **input take + sim step + telemetry encode** (headless loop) | **0** per frame after warm-up | all | counting allocator, §4.4 |
| N6 | Renderer + UI allocations per frame | tracked metric with a ceiling set after the first measurement (egui and wgpu-core allocate per frame by design) | H1 | §4.4 |
| N7 | Application heap (excl. GPU driver mappings), default scenario | ≤ 64 MB steady state | all | §4.4 |
| N8 | Cold start to first interactive frame | ≤ 1.5 s H1, ≤ 3 s H3 | H1, H3 | §4.5 |
| N9a | Telemetry throughput, research configuration | 3 sources × 8000 events/s + 1000 frames/s + 1000 `sim_camera`/s, **zero drops**, ≤ 1 % of one core (recorder + writer) | H1, H3 | §4.6 |
| N9b | Data loss on process kill | ≤ `chunk_flush_ms` (100 ms) + writer backlog at kill time (backlog normally ≤ one group; maximum reported in `session_end`) | all | SIGKILL test, §4.6 |
| N9c | Data loss on power loss | ≤ `fsync_ms` (1000 ms) + `chunk_flush_ms` + backlog | H1 | VM hard reset / dm-log-writes replay, §4.6 |
| N10 | Replay / re-simulation | ≥ 100× real time, headless, single core | H1 | §4.7 |
| N11 | Input-to-photon latency | **no target until a baseline exists** (EXPERIMENT_PLAN E5); then regression budget ≤ +0.5 ms median between releases | H1, H2 | E5 |
| N12 | High-polling-rate robustness (Windows) | with an 8 kHz source: N1/N2 still met; Σcounts conserved vs ground truth | H1 (Windows) + 8 kHz device | §4.8 |

## 3. Browser targets

| ID | Metric | Target | Procedure |
|---|---|---|---|
| W1 | Compressed transfer size (wasm + JS + required assets) for the trainer | ≤ 3 MB goal, ≤ 6 MB hard ceiling (Brotli where the host serves it, else gzip) — measured, then single vs dual artifact decided | CI size check every build |
| W2 | Time to interactive, first visit, 50 Mbit/s simulated, H1 | ≤ 4 s | Playwright + throttling, §4.9 |
| W3 | Time to interactive, cached (service worker) | ≤ 1.5 s | §4.9 |
| W4 | rAF cadence, foreground | equals display refresh ± 1 % where the browser runs rAF at native refresh; otherwise the browser's cap is reported as a limitation (Safari default ≈60 Hz documented for older versions; current default unverified) | §4.2 |
| W5 | Frame CPU time (rAF callback start → submit) | same 25 % budget rule as N1, egui share reported | §4.1 |
| W6 | Peak JS + wasm memory, 30-min session | ≤ 512 MB; recording streams to OPFS (no in-memory accumulation) | §4.4 (`performance.measureUserAgentSpecificMemory` where available, else wasm memory size) |
| W7 | Export throughput, 30-min session | ≥ 20 MB/s to a file stream | §4.6 |
| W8 | Background behaviour | gap annotation durable in OPFS ≤ 50 ms after the `visibilitychange`/`pagehide` event's timestamp (verified by reading OPFS back); no samples fabricated while hidden | §4.2 |
| W9 | Device/context loss | interactive again (or a clear message) within 2 s; trial aborted and marked; session data preserved | fault injection, §4.10 |
| W10 | Storage exhaustion | with the quota forced low (CDP `Storage.overrideQuotaForOrigin`), zero corrupt chunks, session ends `storage_error`, recovered session validates | §4.6 |

## 4. Procedures

### 4.1 CPU frame cost
`profiling`-facade spans with Tracy (GUI version pinned to tracy-client's); in-app counters write per-frame CPU duration into `frame` records, so every session doubles as a benchmark. Report p50/p99/p99.9 and the egui pass share.

### 4.2 Frame pacing
Native: CPU frame-start timestamps (session clock) plus GPU timestamp queries where supported; Windows cross-check with PresentMon; external photodiode for ground truth. Browser: rAF timestamps, headed only (headless runs a synthetic ≈60 Hz). 5-minute runs, foreground, then with synthetic background CPU load, then a hide/show cycle.

### 4.3 Microbenchmarks and handoff
Pure-CPU paths (sensitivity transform, gain step, event decode/encode, analytics kernels) with criterion (wall time) natively and gungraun (instruction counts, Linux CI) for regression detection; wasm microbenchmarks under Node for relative comparison only. Handoff latency (N4b) is measured on hardware from the receipt time `t_ns` to the apply time recorded in a debug build.

### 4.4 Memory
A counting `#[global_allocator]` asserts zero allocations across N steady-state iterations of the headless input→sim→encode loop (N5); renderer/UI allocations are counted per frame and tracked (N6). Heap profiles with dhat. RSS reported but not targeted (drivers dominate).

### 4.5 Startup
Process start (native) / navigation start (web) → first frame presented with input enabled; 20 runs; median and max; cold and warm.

### 4.6 Telemetry durability and throughput
Synthetic generator at the N9a rates for 30 minutes (sequence-gap check for drops, CPU and bytes/event); SIGKILL at random times → recovery validates, loss ≤ N9b; power-loss simulation in a VM (hard reset) or with `dm-log-writes` replay → loss ≤ N9c. Browser: same generator into the OPFS worker; tab closed at random times; quota exhaustion (W10).

### 4.7 Replay
Headless replay of the regression corpus; wall time vs session duration; hash equality (EXPERIMENT_PLAN E2/E10).

### 4.8 High polling rate
A Windows machine with an 8 kHz mouse or a high-speed (480 Mbit/s) programmable HID device (e.g. Teensy 4.x — full-speed RP2040/nRF52840 devices are limited to 1 kHz); trial with the cursor captured (`RIDEV_NOLEGACY` active) and in menus; N1/N2 plus Σcounts vs a USB capture.

### 4.9 Web load
Playwright drives Chromium and Firefox (WebKit via Playwright and manual Safari on H4); DevTools-protocol throttling where supported; `PerformanceNavigationTiming` + an app mark at the first interactive frame.

### 4.10 Fault injection
Browser: `GPUDevice.destroy()`, `WEBGL_lose_context`. Native: surface lost/outdated on resize, minimise, monitor change.

## 5. Benchmark matrix

✓ = must measure before 1.0; ○ = when hardware is available; — = not applicable.

| Configuration | H1 Linux X11 | H1 Linux Wayland | H1 Windows | H2 Windows | H3 | H4 macOS | Web @ H1 Linux (Chromium) | Web @ H1 Linux (Firefox) | Web @ H1 Windows (Chrome, Firefox) | Web @ H4 (Safari) |
|---|---|---|---|---|---|---|---|---|---|---|
| Vulkan Fifo | ✓ | ✓ | ✓ | ○ | ✓ | — | — | — | — | — |
| Vulkan Mailbox / Immediate | ✓ | ✓ | ✓ | ○ | ✓ | — | — | — | — | — |
| DX12 | — | — | ✓ | ○ | ○ | — | — | — | — | — |
| Metal | — | — | — | — | — | ○ | — | — | — | — |
| WebGPU | — | — | — | — | — | — | ✓ **Wayland session only** (NVIDIA ≥535.183.01); X11 = fallback | — (not shipped on Linux) | ○ (if Windows is installed) | ○ |
| WebGL2 fallback | — | — | — | — | — | — | ✓ | ✓ | ○ | ○ |
| 1 kHz mouse | ✓ | ✓ | ✓ | ○ | ✓ | ○ | ✓ | ✓ | ○ | ○ |
| 8 kHz source (N12) | ○ | ○ | ✓ (needs an 8 kHz device) | ○ | — | ○ | ○ | ○ | ○ | ○ |

## 6. Regression policy

- Size (W1), allocations (N5, N6 ceiling), instruction counts (N4a), replay determinism and render–sim consistency (EXPERIMENT_PLAN E11) are enforced in CI.
- Frame pacing, latency and high-polling robustness are measured on the bench rig per release and committed as fixtures; regressions block release, not merges.
