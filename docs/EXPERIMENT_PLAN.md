# EXPERIMENT_PLAN — KineForge

Status: research-phase plan, 2026-11-13. Nothing here has been run yet. Every
experiment states what it can and cannot establish. Results are recorded in the
telemetry format of [DATA_SPECIFICATION.md](DATA_SPECIFICATION.md) with
data provenance per DATA_SPECIFICATION.md §3 (`input_level`, `evidence_class`); claims and constants derived from results carry a `claim_status` (§0.2).

## 0. Ground rules

### 0.1 Separation of test kinds

| Kind | Example | Needs hardware? | Needs a human? | Runs in CI? |
|---|---|---|---|---|
| Pure math | cm/360, FOV conversion, fixed-point gain | no | no | yes (native + wasm) |
| Simulation | seeded scenario produces identical target sequence | no | no | yes (native + wasm) |
| Platform integration | evdev/Raw Input/Pointer Lock capture path delivers injected counts | virtual device or browser | no | partly (Linux uinput in a privileged runner; headless browser) |
| Hardware-dependent | count fidelity with a real mouse, click-to-photon latency | yes | no | no — bench rig, results committed as fixtures |
| User performance | flick/tracking studies | yes | yes | no |

A result from one kind is never presented as evidence for another (e.g. a
unit test of the sensitivity formula does not establish that a game uses it).

### 0.2 Claim status (`claim_status`; the single definition used by every document)

- **VERIFIED** — primary documentation or source code, the local system observed directly, or our own code under test.
- **EXPERIMENTALLY OBSERVED** — measured by a procedure in this file; reported with n, spread, and the rig used.
- **INFERRED** — derived from verified facts or observations by stated reasoning or a named model.
- **COMMUNITY-ESTABLISHED** — widely used community value with no primary source found.
- **UNKNOWN** — not established.
- **REQUIRES FURTHER VALIDATION** — single observation, unreplicated, rig not yet characterised, or sources conflicting / weakly sourced.

### 0.3 Common controls

- Record the full environment block (OS build, compositor/session type, browser version, GPU + driver, refresh rate, mouse model + firmware + CPI + polling rate, OS pointer acceleration state) in the session manifest. Experiments without it are not admissible.
- OS pointer acceleration off (Windows EPP off; libinput `flat`; macOS acceleration cannot be fully disabled through public UI — record the value of `com.apple.mouse.scaling`), unless the experiment is *about* acceleration.
- Seeded RNG for every stimulus sequence; the seed is in the manifest.
- Warm-up block excluded from analysis; block order counterbalanced (ABBA or Latin square).
- Pre-registration lite: hypothesis, primary metric and analysis written into the experiment config **before** data collection; deviations recorded.

---

## E0 — Clock and timestamp characterisation (foundation for everything)

**Question.** For each capture path, what is the timestamp's clock domain, resolution, monotonicity, and offset to the app's monotonic clock?

**Procedure.**
1. For each source (evdev `input_event.time`, Raw Input + QPC-at-receipt, Wayland `utime`, macOS `mach_absolute_time`/`NSEvent.timestamp`, browser `event.timeStamp`, `performance.now()` in handler, app `Instant::now()` at dequeue), collect ≥100 000 events while moving the mouse continuously at max polling rate.
2. Compute: smallest non-zero Δt (resolution), histogram of Δt (should show polling period, e.g. 1.0 ms / 0.125 ms), count of non-monotonic pairs, offset and drift between source timestamp and app clock (linear regression, residual SD).
3. Browser: repeat with and without cross-origin isolation (COOP/COEP) to observe timer coarsening.

**Output.** Per-path `ClockDomain` record (resolution_ns, jitter SD, drift ppm) stored as a fixture; this populates the clock-domain fields `resolution_ns`, `offset_uncertainty_ns` and `drift_ppm` (DATA_SPECIFICATION.md §2.1).

**Limits.** No path exposes the device's own report-generation time; the earliest timestamp is host-side receipt (kernel or OS). USB polling quantisation (≤1 ms at 1 kHz, ≤125 µs at 8 kHz) is an irreducible floor on knowing *when* motion happened.

---

## E1 — Count fidelity per capture path

**Question.** Does each capture path deliver exactly the counts the device (or a virtual device) emitted — no scaling, rounding, acceleration, loss, or reordering?

**Sources of known motion (in order of trust).**
1. **Programmable hardware**: a microcontroller enumerating as a USB HID mouse (e.g. RP2040/nRF52840 running a small TinyUSB/QMK/Zephyr sketch — Open Directional DPI already targets QMK and Zephyr) replaying a scripted count sequence at a fixed report rate. This is the only source that exercises the full USB→OS path with known ground truth.
2. **Passive L1 tap of a real mouse**: Linux hidraw (reading does not take the mouse from the OS). **Available on this machine now**: `/dev/hidraw2` (Glorious receiver mouse interface) is readable via the existing udev rule.
3. **Kernel virtual device**: Linux `uinput` — the user already has read/write access to `/dev/uinput` through an ACL. Exercises evdev→libinput→X11/Wayland→browser, but not USB.
4. **OS injection**: Windows `SendInput`, macOS `CGEventPost`. Injected events take different paths from hardware events and are flagged by the OS; label them `injected_test_stream` and treat results as indicative only.

**Procedure.** Emit fixed patterns: unit steps (±1 count), large single reports (±127, ±32767 where the report descriptor allows), sinusoids, random walks with a recorded seed, diagonal and axis-aligned bursts, and bursts at 125/500/1000/4000/8000 Hz. Capture simultaneously (where possible) through every available path. Compare per-report and cumulative sums.

**Metrics.** Cumulative count error, per-event scaling factor (fit), fraction of events coalesced, max reorder distance, dropped events, sub-count fractional residue (browsers may report fractional movement on scaled displays).

**Expected outcomes to confirm or refute** (predicted from source code; not yet measured):
- evdev and Xorg `XI_RawMotion` (libinput unaccelerated, which is documented as device units, not DPI-normalised) equal HID counts per report; all-zero reports are absent from evdev.
- Windows Raw Input relative packets equal HID counts one-to-one in the foreground; background listeners are coalesced on Windows 11.
- Browser `movementX` on Linux is accelerated (adaptive profile here) — the ratio to HID counts rises with speed; under unadjusted lock on Windows it equals `lLastX`.

**Analysis: dual-source ratio test.** Over sliding windows compute r = Σ|Δ_test| / Σ|Δ_L1| and regress r on L1 speed: r ≡ 1 clean; constant r ≠ 1 linear scaling (DPR, surface scale); r rising with speed = acceleration; equal sums with per-event mismatch = coalescing/resampling; missing sums = drops. Add quantisation fingerprints (non-integer values from an integer source ⇒ scaling; µs timestamps ≡ 0 mod 1000 ⇒ ms source).

**Limits.** Firmware-side processing (angle snapping, smoothing, lift-off, motion sync, sensor DPI quantisation) happens before any host path and is invisible to it; only an instrumented firmware can observe it.

---

## E2 — Sensitivity and camera-rotation verification (KineForge itself)

**Question.** Does KineForge rotate the camera by exactly `counts × yaw_per_count × sensitivity` (plus configured directional gain), with no frame-rate dependence?

**Procedure.** Replay recorded count streams through the simulation at simulated frame rates 30–1000 Hz and with different **delivery** batching (how many events each frame takes). Events are always applied one by one in `seq` order — never pre-summed — because directional gain and the pitch clamp are path-dependent. Assert final yaw/pitch bit-identical across all frame rates and batchings (also with gain and clamping active), and that the integer unit totals equal the sum of the stream. Repeat on native x86-64, native ARM64 and wasm32.

**This is a pure simulation test and belongs in CI.**

---

## E3 — Black-box yaw-per-count measurement of a reference application (e.g. Aimlabs)

**Question.** At a given in-app sensitivity setting, how many counts produce a 360° turn?

**Status: blocked pending a decision.** Automated count injection plus automated frame capture is a **grey zone** under the Aimlabs Terms (AIMLABS_RESEARCH.md §6.3). Preconditions: you launch Aimlabs yourself, guest/offline mode, no score submission, and legal review or Statespace's written consent (STACK_DECISION.md §6 item 7). Human-operated variants (real mouse on a rail, passive host-side logging, manual reading of results) are allowed now. Never any code inspection.

**Procedure (360° image-return method).**
1. Fix CPI-independent ground truth by using a programmable count source (E1 source 1 or 3).
2. In the reference app, face a distinctive static scene; capture a reference frame.
3. Inject counts in small fixed increments (e.g. 1 count per report at low rate to avoid app-side smoothing) until the captured frame best matches the reference again after one full turn (normalised cross-correlation over a horizontal strip; sub-pixel peak interpolation).
4. counts_per_360 = injected total at best match; yaw_per_count = 360° / (counts_per_360 × sens).
5. Repeat for ≥5 sensitivity values (e.g. 0.25, 0.5, 1, 2, 4); fit yaw_per_count×sens vs sens to detect non-linearity; repeat each ≥3 times.

**Precision.** Limited by angular resolution of the image match (≈ hFOV / horizontal pixels per pixel, improved by sub-pixel interpolation) and by whether the app quantises rotation. Report ± in counts per 360.

**Confounders.** Proton/Wine raw-input emulation (under X11 Wine derives "raw" mouse motion from X input events — verify which), OS acceleration, app-side smoothing, frame drops during capture, vertical drift. Pitch measured separately (clamped at ±90°, so use a partial-turn method: rotate a known angle using the established yaw constant and measure landmark displacement).

**Output.** INFERRED constants with experiment ID, rig description, n, and confidence interval. Never published as "the" constant without the conditions.

---

## E4 — FOV measurement of a reference application

**Procedure.** With yaw_per_count known from E3, rotate by small known angles θ and measure the horizontal pixel displacement x of a far landmark near screen centre. Under rectilinear projection x = f·tan(θ) with f = (W/2)/tan(hFOV/2); fit f by least squares over θ ∈ [−30°, 30°]; derive hFOV and check aspect-ratio scaling (Hor+ vs Vert−) by repeating at 16:9, 16:10 and 4:3 resolutions.

**Limits.** Lens distortion or non-rectilinear post-processing would show as residual structure; report it rather than forcing the model.

**Status:** with automated injection/capture this inherits E3's gate; a human-operated variant (manual rotation with markers in a self-built Creator Studio map, screenshots taken by hand) is allowed now.

---

## E5 — Input-to-photon latency

**Question.** Time from a physical input event to the first visible change on screen, per platform, per present mode, per browser.

**Rig.** Microcontroller that (a) enumerates as USB HID mouse, (b) emits a click or a single large motion report at a logged time, (c) reads a photodiode taped to the screen region that changes (KineForge renders a full-white patch on click). Optional cross-check: high-speed camera ≥960 fps, or NVIDIA LDAT-class hardware if available.

**Procedure.** ≥500 trials per condition, randomised inter-trial interval (to decorrelate from refresh phase). Conditions: native Fifo/Mailbox/Immediate; frame cap on/off; fullscreen vs windowed; browser Chromium/Firefox/Safari; WebGPU vs WebGL2.

**Metrics.** Median, p5/p95, distribution shape (expect a uniform component of one refresh period).

**Limits.** Includes display processing/response time; compare only within one monitor. Never report software-only timestamps as "input latency" — they measure host processing only and are labelled `HostProcessingLatency`.

---

## E6 — Frame pacing and host timing

**Procedure.** Record per-frame: CPU frame start, input dequeue time, sim step count, submit time, and where available present/GPU timestamps (wgpu timestamp queries native; browser `timestamp-query` if exposed, otherwise rAF timestamps only). 5-minute runs per condition; also with background CPU load. Browser runs in foreground and after a visibility change to measure throttling and recovery.

**Metrics.** Frame-time mean, SD, p99, p99.9, count of frames > 1.5× refresh period, input-age-at-sample distribution.

---

## E7 — Human aim-performance experiments

**Designs.**
- **Flick (discrete pointing).** Targets at controlled angular distances D and angular widths W, directions drawn from a balanced set (e.g. 16 or 24 directions × distances), order seeded. Metrics: movement time, reaction time (stimulus onset → first movement above threshold), endpoint error (signed, along and across the movement axis), overshoot, number of submovements, Fitts throughput with effective width W_e = 4.133·SD_endpoints (ISO 9241-411 practice; Soukoreff & MacKenzie 2004).
- **Tracking (continuous).** Target trajectories from seeded smooth noise (sum of sinusoids with non-harmonic frequencies, so the human cannot learn the period) at controlled speed spectra. Metrics: RMS angular error, time-on-target, lag via cross-correlation, gain/phase per frequency.
- **Switching / reactive.** Target jumps at random times; metric: reaction time, re-acquisition time.
- **Dynamic flick (moving targets).** Seeded target speed and velocity direction relative to the flick vector (toward / away / tangential); metrics: endpoint error relative to the target at click time, signed lead/lag along target velocity, time-to-hit vs speed. Target velocity is an explicit factor.

**Comparisons (e.g. sensitivity A vs B, gain curve on vs off).** Within-subject, blocks counterbalanced, ≥2 sessions on different days to separate learning from treatment. Analyse with mixed-effects models (participant random effect, block/session fixed effects); report effect sizes with confidence intervals, not just p-values. Do a power analysis from pilot variance before claiming "no difference".

**Separating error sources.** Hardware/OS noise is measured by E0/E1 and is expected to be much smaller than human variability — confirm by comparing E0/E1 residuals with the pilot within-subject SD before relying on it; frame-rate effects are tested as an explicit factor; scenario randomness is removed by giving all conditions identical seeded stimulus sequences.

**Limits.** Learning, fatigue, motivation; small-n studies (often n=1, the developer) generalise only to that person — label as such.

---

## E8 — Directional DPI calibration validation

**Procedure.**
**Validity rule (normative).** Fitting a directional-gain profile and exporting it to firmware is allowed only from `device_report`, `os_raw_counts`, `os_unaccelerated` with acceleration verified absent, or `browser_locked_unadjusted` sources. Sources with a constant unit scale (DIP, DPR, unverified macOS units) are allowed only with mean-preserving normalisation and the scale recorded. **`browser_locked` (accelerated — every Linux browser) is never used for fitting**: acceleration makes gain speed-dependent, and flick speed varies with distance and direction, so acceleration would be fitted as directional error. Such data may be shown as indicative only; profile export is refused (`kf-calib`).

**Procedure.**
0. **Validity check (runnable on this machine now):** record the same flick session simultaneously from `hidraw2` (L1) and from Chromium/Firefox Pointer Lock (accelerated); fit both; compare the profiles. The difference quantifies the error an accelerated stream would introduce.
1. Collect flick/tracking data with gain curve off; fit the directional gain model (Open Directional DPI's procedure — see OPEN_DIRECTIONAL_DPI_AUDIT.md) per participant.
2. **Retrospective check**: hold out 30 % of trials; check that the fitted curve reduces direction-dependent systematic endpoint bias on held-out data (computed by re-applying gain to recorded counts — valid only for the open-loop part of a movement, because the human would have corrected differently; state this).
3. **Prospective check**: new sessions with curve on vs off (blinded order where possible); compare direction-dependent bias and overall throughput.
4. **Firmware conformance**: the profile exported to firmware must produce output counts identical to the host Rust implementation on shared golden vectors (bit-exact).

---

## E9 — Scoring

- **KineForge scoring**: defined by spec, verified by unit tests with hand-computed fixtures (pure math).
- **Reference-app scoring (black box)**: record visible score, hits, misses, duration, reaction times from the results screen over many runs (manual transcription is allowed now; automated capture inherits E3's gate); regress score on candidate explanatory variables; report as INFERRED with fit quality, never as the app's formula. Results are stored as `app_result` records (DATA_SPECIFICATION.md §6.5, §7.1).

---

## E10 — Cross-target equivalence

**Procedure.** A corpus of recorded input streams + scenario seeds is replayed on Linux x86-64, Windows x86-64, macOS ARM64 and wasm32 in Chromium and Firefox. Simulation state hashes per tick must match bit-exactly on the deterministic path; derived analytics must match within stated tolerances. Telemetry exported on one target must import on every other (round-trip hash).

---

## E11 — Render–simulation geometric consistency (KineForge itself, CI)

**Question.** Does the f32 renderer draw targets and the crosshair exactly where the f64 simulation says they are, on every backend?

**Procedure.** Headless wgpu renders targets at known angular offsets (Δψ, Δθ) and angular radii at aspect ratios 16:9, 16:10, 4:3, 21:9, at even and odd resolutions, and (web) DPR 1 and 1.25. Read back pixels; assert the target centroid equals W/2 + f·tan(Δψ) (f = (W/2)/tan(hFOV/2)) within 0.5 px, the silhouette radius matches the angular radius, and the crosshair offset is ≤ 0.5 px (the pixel-snap offset is recorded in `render_state` records, DATA_SPECIFICATION.md §6.1). Backends: Vulkan, DX12, Metal, WebGPU (SwiftShader) and WebGL2.

**Why.** Hit tests run in the simulation; if the picture disagrees, users are scored on something they did not see.

---

## P — Platform input experiment catalogue (IDs prefixed `X-` to avoid clashing with layers L1–L7, milestones M0–M13 and hardware classes H1–H5)

Detailed procedures and source references: `research/notes/input.md` §9 and
§1/§4.5/§6. Tags: **[local-now]** runnable on this machine without any
permission change; **[perm]** needs a user decision; **[hw]** needs other
hardware or an OS not available here.

| ID | Question | Tag |
|---|---|---|
| **X-L1** | Xorg `XI_RawMotion` raw values vs `hidraw2` counts, per report and in sum; X timestamps ms | local-now |
| **X-L2** | acceleration contamination of cursor and browser paths vs `hidraw2` (adaptive profile active) | local-now |
| X-L3 | evdev timestamp = kernel flush time; hidraw-read vs evdev latency | perm |
| X-L4 | evdev ring overflow (`SYN_DROPPED`) at 8 kHz under load | perm + hw |
| X-L5 | Wayland `dx_unaccel` fidelity and `utime` base on GNOME/KDE/sway, fractional scales, Xwayland | hw (Wayland session) |
| X-L6 | libinput flat profile at speed 0 ⇒ accelerated == unaccelerated == counts | perm (user changes a setting) |
| X-L7 | libinput ≥ 1.30 plugin and HID-BPF contamination detection | hw |
| X-M1 | macOS field 170, GCMouse and `NSEvent.deltaX` units vs IOHID counts across pointer-speed settings | hw (Mac) |
| X-M2 | Input Monitoring scope (mouse interface vs vendor-only interface) | hw |
| X-M3 | GCMouse callback rate vs IOHID report rate at 1/4/8 kHz | hw |
| X-M4 | macOS timestamp domains (IOHID AbsoluteTime vs NSEvent vs CGEvent) | hw |
| X-W1 | Raw Input pass-through vs USB capture, 1:1 packets at 1/2/4/8 kHz | hw (Windows) |
| X-W2 | Windows 11 background coalescing rate and losslessness | hw |
| X-W3 | Raw Input / GameInput / WinRT / WM_MOUSEMOVE invariance under EPP and speed changes | hw |
| X-W4 | QPC-at-receipt vs GameInput µs timestamps; GameInput readings per report | hw |
| X-W5 | filter-driver detection (RawAccel on a test machine) | hw |
| X-W6 | RDP/VM absolute packets | hw |
| X-H1 | Windows: mouse collection unreadable via hidapi; vendor collection readable | hw |
| X-H2 | USB bus capture vs every L1/L2 source at 1–8 kHz | perm (usbmon needs root) / hw |
| X-H3 | firmware angle snapping / lift-off visible in L1 counts (rig at 0°, 3°, 45°) | hw (rig) |
| X-H4 | hardware HID injector as universal ground truth for all layers incl. browsers | hw (RP2040/Teensy) |
| X-R1 | winit X11 `DeviceId` collapsing multiple mice | local-now (needs a second mouse) |
| X-R2–X-R7 | winit RDP bug, SDL/winit CPU at 8 kHz, SDL X11 timestamp leak, SDL interpolation error, Wayland second queue on winit's display, winit `is_cursor_lock_raw` accuracy | mixed |
| **X-B1** | `unadjustedMovement` rejected on Linux (Chromium 156, Firefox 159) | local-now |
| **X-B2** | browser `movementX` vs `hidraw2` ratio vs speed | local-now |
| **X-B3** | DPR 1.25 (throwaway profiles with forced scale factor) | local-now |
| **X-B4** | page zoom 200 % | local-now |
| **X-B5** | event timestamp grids with/without COOP/COEP | local-now |
| X-B6 | Windows 1/4/8 kHz: coalesced entries per `WM_INPUT`, Σ conservation | hw |
| X-B7 | Chrome re-lock cooldown (1250 ms) and rate limit; Firefox/Safari behaviour | local-now (Chrome/Firefox) / hw (Safari) |
| X-B8 | Windows absolute pointers under RDP/VM in browsers | hw |
| X-B9 | Firefox Wayland truncation and µs-as-ms timestamp suspicion | hw (Wayland session) |
| X-B10 | macOS browser unadjusted units vs IOHID | hw |
| **X-B11** | `pointerrawupdate` vs `pointermove` delivery relative to rAF | local-now |

**Cheapest first pass (recommended):** one recording session on this machine
with `hidraw2` as ground truth, run against `XI_RawMotion`, XI2 cursor motion
and Chromium/Firefox Pointer Lock (X-L1, X-L2, X-B1–X-B5, X-B11). It tests the predicted
XI2-raw = counts identity, the browser acceleration curve under the adaptive
profile, DPR/zoom scaling and browser timestamp grids — with no permission
change. Needs only small throwaway capture scripts (to be written when you
approve moving past research).

## Measurement limitations (summary)

1. No host software observes sensor-internal timing or firmware processing.
2. Browser movement data is OS/browser-processed even under Pointer Lock with `unadjustedMovement`; it is never labelled as device counts.
3. Timestamps are host-receipt times with path-specific resolution (E0).
4. Software-only latency figures exclude display and USB latency.
5. Black-box measurements of third-party apps yield conditional, inferred constants.
6. Human-performance results are noisy; effects smaller than the measured within-subject SD need correspondingly larger samples.
