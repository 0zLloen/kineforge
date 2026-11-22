# OPEN_DIRECTIONAL_DPI_AUDIT — for KineForge integration

Audit date: 2026-11-13. Read-only; nothing in the ODD repository was modified.
Experiments ran on a `git archive HEAD` copy in a temporary scratch directory.
The probe sources are preserved in [`research/odd-probes/`](../research/odd-probes/)
so every number below can be reproduced.

Paths are relative to the ODD checkout unless stated; `file:N` = line N.

## Addendum (2026-11-22)

- Since the audit (`c177d23`), ODD gained two commits on branch `fix/led-chords` — `a2594b6` (debug-only memory-read build, `peek_dump.py`) and `3ea27cf` (stock LED-chord fix). Both touch only `adapters/glorious-cm2856/`; **`core/`, `spec/` and the vectors are unchanged**, so every algorithmic finding below still holds.
- Hardware correction: "CM2856" is the Compx project name, not the MCU. The Series One PRO Wireless uses a **Nordic nRF52840** with a resident Compx USB HID bootloader (documented in `mouse-firmware-re`, `platforms/compx-nrf52/`). The ODD patch has run on the retail unit since 2026-11-13, so `SIMULATE_DELTA` on the actual mouse is available as an extra parity check for `kf-dirgain` (M2), with the caveat that it mutates live engine state.

## Executive summary

- **Identity verified.** `/home/lloen/src/open-glorious-one-firmware` **is** the Open Directional DPI repository (`origin` = `git@github.com:0zLloen/open-directional-dpi.git`). The directory name is historical (the project began as Glorious custom-firmware research). The checked-out branch `refactor/repo-layout` (`c177d23`) has the same tree as upstream `main` (`3b12333`, merge of PR #10); local `main` (`a7f41f4`) is one merge commit behind because it was never fetched. `feat/shift-profiles-radio` is fully merged (its tree equals `main`'s) and is checked out in a worktree under `/tmp`.
- **The core is excellent material for KineForge**: ~830 lines of freestanding C, integer-only (Q8.8 / Q16.16), no malloc/stdio/globals, caller-owned 64-byte state, wire formats packed byte-by-byte little-endian, C99/C11 clean under strict warnings, no UB found under ASan+UBSan fuzzing (4 M calls).
- **Recommendation: port the engine to a `no_std` Rust crate (`kf-dirgain`, ≈350 lines of logic), keep the C engine as the reference oracle, and make the spec + golden vectors the shared contract** (option B governed by C + D, §8). Calling the C library through FFI would require a clang/wasi-sdk toolchain for every browser build, which this project should not impose.
- **Five issues matter for KineForge** (§6): the direction-smoothing window saturates above L ≈ 31 counts (reproduced independently); saturated output is carried (capped at ≈32767 counts — anything beyond is dropped) and released later, even across profile changes; adapters clamp at inconsistent points; there are two divergent calibration analyses and the browser calibrator silently falls back to accelerated input; and only the stateless `compute_scale` function has cross-language golden vectors.

## 1. Identity, branches, history

| Ref | Commit | Notes |
|---|---|---|
| `refactor/repo-layout` (HEAD) | `c177d23` | equal to `origin/refactor/repo-layout`; tree identical to upstream `main` |
| upstream `origin/main` | `3b12333` | merge of PR #10; compare `c177d23...3b12333`: ahead 1, behind 0, **0 files differ** |
| local `main` | `a7f41f4` | merge of PR #9; stale (not fetched) |
| `feat/shift-profiles-radio` | `e2dddf7` | local + origin; tree == `main`; merged |

`main..refactor/repo-layout` changes no engine code: it moves the QMK test and `calibrate.html` (to `web/`), adds an `aimlabs` provider alias (`calibration/cli.ts:156`, an alias of the MouseTester-log provider, used for logs recorded while playing a trainer), adds `adapters/glorious-cm2856/README.md`, and rewrites docs/CI paths. `core/`, `spec/` and vectors are unchanged.

The project's first commit is `4df8a22` (2026-11-05): it is 8 days old and actively changing. Core changes since then: `d0762df` (estimator wrap fix, atan constant, `process_motion`), `0474ba9` (`ONLY_AT_BASE_DPI`), `21ea252` (clock in capabilities).

## 2. Structure

| Area | Files (LOC) | Language |
|---|---|---|
| Core engine | `core/directional_dpi.{h,c}` (311 + 827) | C (C99/C11 clean) |
| Spec | `spec/directional-dpi-v1.md` (205), `spec/hid-protocol-v1.md` (181) | Markdown; spec §0 states the code + vectors are authoritative |
| C tests / vectors | `tests/test_directional_dpi.c` (17 tests), `tests/gen_vectors.c`, `tests/vectors/scale_vectors.json` (2592 vectors) | C / JSON |
| TS mirror + WebHID client | `integrations/openmouse/src/directional-dpi.ts` (1076), `mock-hid.ts`, tests (21 suites) | TypeScript (strip-types, no build) |
| Calibration | `calibration/{cli.ts, core/*, providers/*}` (~900) + tests + CSV fixtures | TypeScript |
| Browser calibrator | `web/calibrate.html` (593, self-contained, GitHub Pages) | HTML/JS |
| Device tool | `tools/odd_profile.py` (+ tests) | Python stdlib (+ optional hidapi) |
| Adapters | `adapters/linux-evdev`, `adapters/qmk`, `adapters/zephyr` (+ DT binding, Kconfig), `adapters/glorious-cm2856` (ROM patch, Unicorn verifier) | C / Python |

Build: plain Makefiles; Zephyr via west/CMake; TS runs with `node --experimental-strip-types`, `tsc --noEmit` for type-checking. Licence: **MIT** (SPDX headers everywhere). CI (`.github/workflows/ci.yml`): binary/vendor guard, `tsc`, C tests, **vector regeneration with `git diff --exit-code`**, evdev selftest, QMK against real QMK 0.34.8 headers, Glorious patch size/link guard with `gcc-arm-none-eabi`, TS and Python suites; `zephyr.yml` runs twister on `native_sim` (Zephyr v4.5.0, v3.7.2) and a ZMK `nice_nano` build; `pages.yml` publishes the calibrator.

Re-run during the audit (scratch copy): C 17/17 (also under ASan+UBSan), vectors regenerate byte-identical, OpenMouse TS 21/21, calibration 15/15, Python tests OK, evdev selftest OK, QMK raw-HID test OK. Not run: Zephyr/ZMK (no west) and the Unicorn patch verifier (no stock image).

## 3. The algorithm, exactly

### 3.1 Number formats

- **Q8.8 `u16`** — eight direction multipliers `M[0..7]` and global gain `G` (`core/directional_dpi.h:31-34`), range 16..1024 (0.0625×..4×). `sanitize` clamps (`.c:40-48`); `deserialize` **rejects** out-of-range values (`.c:226-240`).
- **Q16.16** — scale `S` (`u32`, max 16.0× = 1048576); per-axis remainders `rem_x/rem_y` (`i32`); intermediates `i64/u64`.
- `dir_x_q16_16 / dir_y_q16_16` are **misnamed**: they hold counts × 256 (Q8), bounded to |d| ≤ 16383 (`.c:538-571`).
- **No floating point** anywhere in the core (checked in x86-64 and ARM disassembly).

### 3.2 Direction → octant (`ddpi_compute_scale_q16_16`, `.c:347-461`)

```
if profile == NULL or (dx,dy) == (0,0) or is_identity(profile): return (65536, octant 0)
ax = |dx|, ay = |dy| as u32   (−32768 → 32768)
octant by signs and |dx| vs |dy| (zero counts as ≥ 0), u = max, v = min:
  dx≥0,dy≥0: ax≥ay → o0 (E,SE) else o1 (S,SE)      dx<0,dy≥0: ay≥ax → o2 (S,SW) else o3 (W,SW)
  dx<0,dy<0: ax≥ay → o4 (W,NW) else o5 (N,NW)      dx≥0,dy<0: ay≥ax → o6 (N,NE) else o7 (E,NE)
```
Octants are in the HID clockwise frame (+Y toward the user). Bins are exactly 45° apart from a fixed 0° origin; there is no rotation offset.

### 3.3 Position within the octant (`.c:431-452`)

```
r = (v ≥ u and u > 0) ? 65536 : (v > 0 ? floor((v << 16) / u) : 0)
t = r
if not LINEAR_OCTANT_RATIO and 0 < r < 65536:
    t = min(65536, r + floor(22787 · r · (65536 − r) / 2^32))      # (4/π)·atan(x) ≈ x + 0.3477·x(1−x)
```
Exhaustive sweep (v ∈ [0, 32768], u = 32768): max angle error **−0.2150°** at r ≈ 42366; the linear mode errs up to 4.07°; `t` is monotonic in `r`; 0.3477 is near the minimax coefficient for this form (best 0.2144°). Matches spec §4.

### 3.4 Interpolation and gain (`.c:454-460`, frame mapping `.c:338-345`)

```
M(k) = FRAME_SENSOR_CCW ? mult[(8−k) & 7] : mult[k]
Mc = M(card) << 8 ; Md = M(diag) << 8                      # Q16.16
Mdir = Mc + (((i64)(Md − Mc) · t) >> 16)                   # arithmetic shift: floor toward −∞
S = ((u64)Mdir · G) >> 8                                   # u32 Q16.16
```
Linear in (approximate) angle between neighbouring 45° nodes: C⁰-continuous around the circle, not C¹.

### 3.5 Direction estimator (`update_direction_estimator`, `.c:517-572`)

```
L = min(reserved & 0xFF, 128); if L == 0: est = (dx, dy)
s = |dx| + |dy|
next = 256 · Δ
if d ≠ 0: α = floor((2L << 16) / (2L + s)); next += (d · α) >> 16
while |next_x| > 16383 or |next_y| > 16383: next >>= 1      # both axes
d = next; est = (i16) d
```
α = 1/(1 + s/2L) is the first-order (Padé [0/1]) approximation of e^(−s/2L): the old direction decays by ≈1/e per 2L counts of travel. The spec (`spec/directional-dpi-v1.md:92-98`) states this correctly; **the code comment at `core/directional_dpi.c:542-544` ("Second-order rational Padé decay for exp(−step_l1 / L)") is wrong.**

### 3.6 Per-report processing (`ddpi_process_delta`, `.c:574-647`; `ddpi_process_motion`, `.c:649-664`)

```
curve_off = is_identity(p) || (ONLY_AT_BASE_DPI && sensor_dpi ≠ 0 && sensor_dpi ≠ base_dpi)
if curve_off: out = in (−32768 → −32767); state untouched (rem and d persist)
if Δ == (0,0): out = 0, no state change
if LINEAR_2X2: gx = M(E)·G, gy = M(N)·G, h = trunc((M(NE)·G − M(NW)·G)/2)
               sx = dx·gx − dy·h ; sy = dy·gy − dx·h            # i64
else:          est = estimator(Δ); S = scale(est); sx = dx·S; sy = dy·S
per axis (emit_from_q16_with_remainder, .c:482-511):
  if scaled == 0 and raw == 0: return 0 (rem kept)
  if RESET_REMAINDER_ON_REVERSE and sign(raw) opposes sign(rem): rem = 0
  total = scaled + rem
  out = trunc(total / 65536), clamped to ±32767
  rem = clamp(total − out·65536, ±2147418112)                    # excess beyond the clamp is CARRIED
process_motion: if Δ ≠ 0 and (u32)(now − last_motion) ≥ idle_ms: d = (0,0); last_motion = now; then process_delta
idle_ms = (reserved >> 8) · 5, or 50 if 0
```
Notes: `KEEP_REMAINDER_ON_REVERSE` (bit 1) is a no-op; the 2×2 model ignores `M[S]`, `M[W]`, `M[SE]`, `M[SW]`; `set_profile` resets state only for identity profiles (`.c:327-336`); `set_sensor_dpi` clears `rem` and `d` on change (`.c:295-301`).

### 3.7 Helpers
CRC-16/CCITT-FALSE (poly 0x1021, init 0xFFFF, no reflection, no xorout; `.c:63-76`). `normalize_cpi_headroom` (`.c:125-164`) raises sensor CPI so that max(M·G) becomes 1.0 and rescales M; the calibration TS path has its own float version — **not bit-equal and untested against the C one**. `set_ccw_frame` swaps i ↔ 8−i and toggles bit 3.

## 4. Data structures and protocol

**28-byte profile v1** (`.h:114-123`, spec §7), little-endian, packed byte-by-byte:

| Offset | Field |
|---|---|
| 0 | magic `0x4450` (bytes `50 44`) |
| 2 | version = 1 |
| 3 | flags (not validated; `BYPASS` bit 6 could be persisted) |
| 4 | `base_dpi` (≥ 50) |
| 6 | `G` (Q8.8) |
| 8..23 | `M[8]` (Q8.8) |
| 24 | `reserved`: low byte = window L (counts), high byte = idle (5 ms units) |
| 26 | CRC-16 over bytes 0..25 |

No spare space remains: any extension needs profile v2. In-memory layout is identical on x86-64, i386 (ILP32, like wasm32) and Cortex-M4 (profile 28 B, engine 64 B, align 4; no `i64`, pointer or `long` in any struct).

**JSON `open-directional-dpi-v1`** (spec §8): written by TS and Python; Q8.8 = round-half-up(x·256); `toFixed(4)` floats round-trip exactly; multiplier keys are physical HID directions (`E_0` … `NE_315`).

**HID protocol v1** (`spec/hid-protocol-v1.md`; handler `.c:666-827`): one 64-byte unnumbered Feature report on vendor usage page 0xFFFF/0x0000; packet `'D','P', ver, status, seq, 0xDF, opcode, slot, payload[54], CRC16-LE`. Opcodes: 01 GET_CAPABILITIES, 02 GET, 03 SET, 04 SAVE_FLASH, 05 RESET, 06 SET_ENABLED, 07 SIMULATE_DELTA (runs on the **live** engine, mutating its state), 0F ENTER_BOOTLOADER ("BLRS"). Transports: Glorious feature report or 2.4 GHz receiver relay; QMK raw-HID (6 × 32-byte frames); Zephyr board HID handler. **Because this is a vendor-defined usage page, it is reachable from browsers through WebHID** (standard mouse collections are not — see WEB_PLATFORM_RESEARCH.md).

## 5. Calibration, logging and adapters

**Calibration** measures, per flick, the endpoint of the primary ballistic submovement as a radial ratio (projection on target direction / target distance; > 1 = overshoot) (`calibration/core/trajectory-analysis/index.ts:70-158`): speed = step / max(dt, 0.125 ms); endpoint = first sample after peak speed with speed ≤ 0.18·peak and projection ≥ 0.4·target; submovements by 0.20/0.22 hysteresis. Fitting (`directional-error:54-95`, `profile-generation:51-131`): bin by nearest 45° sector of the **target** direction, keep ratios in [0.4, 2.2], median + MAD rejection, gain = clamp(1/median, 0.8, 1.25), circular 3-tap smoothing [0.15, 0.70, 0.15], then normalisation (mean-preserving by default in the CLI; cpi-headroom is the library default). No regression/optimiser; no compensation for the engine's own interpolation or the human's systematic undershoot. Thresholds are heuristic and uncited.

**Input sources**: MouseTester CSV (Windows Raw Input) or `providers/mouse-log/record-evdev.ts` (Linux evdev, ungrabbed); KovaaK's `Stats.csv` kill-time alignment (±30 ms brute force); and `web/calibrate.html`, a center-out task (8 directions × {140, 210, 280} counts) using Pointer Lock `movementX/Y` with `unadjustedMovement: true` — which **silently falls back to accelerated Pointer Lock if refused** (`web/calibrate.html:456-459`, verified) — and **its own different analysis** (Euclidean distance, 45 % gate, ratio in (0.4, 2.0), no MAD). Nothing tests the two analyses against each other. `mouse-log` and `kovaaks` providers have never run on a real session (synthetic tests only).

**Logging**: the only recorder is `record-evdev.ts` (24-byte 64-bit `input_event` layout only; CLOCK_REALTIME timestamps; no `SYN_DROPPED` handling; left button only). No device-side timestamps, no per-report HID logging.

**Adapters**:

| Route | Engine input | Clock | Output / clamping | Status |
|---|---|---|---|---|
| Glorious CM2856 ROM patch | raw PAW3370 burst counts, before USB/RF report | burst-call count (~1 kHz measured on cable) | writes back stock dx/dy | Unicorn-verified, partly hardware-verified; 4864 B patch budget |
| QMK | `report_mouse_t` after QMK's sensor driver | `timer_read32()` | `CONSTRAIN_HID_XY` **after** the engine: in 8-bit mode excess is lost (probe: 2× gain at 100 counts/report emitted 12700 of 20000) | compiled against QMK headers; never run on a device |
| Zephyr/ZMK input filter | `REL_X/REL_Y` per sync frame, clamped ±32767 **before** the engine | `k_uptime_get_32()` | virtual device | native_sim tested |
| Linux evdev → uinput | kernel `REL_*` per `SYN_REPORT` (before libinput), clamped **before** the engine | event time, CLOCK_REALTIME, truncated to u32 ms | uinput clone, grabs source | selftest only |
| Windows/macOS | no runtime filter; patch loader + `odd_profile.py` + WebHID calibrator | — | — | untested on those OSes |

## 6. Issues relevant to KineForge

1. **Direction-smoothing window saturates (reproduced).** Steady-state |d| ≈ 256·(2L+s), so the 16383 bound engages when 2L+s > 64 and each halving shortens the effective memory. Independent reproduction (`research/odd-probes/probe2.c` against `c177d23`), counts of a 90° turn until the estimate crosses 45°:

   | L \ counts/report | 1 | 2 | 4 | 8 | ideal ln2·2L |
   |---|---|---|---|---|---|
   | 16 | 23 | 24 | 24 | 32 | 22 |
   | 32 | 44 | 34 | 28 | 32 | 44 |
   | 64 | 40 | 46 | 32 | 48 | 89 |
   | 128 | 43 | 50 | 36 | 32 | 177 |

   The window is faithful only for L ≲ 31 at 1 count/report (less at larger steps), is non-monotonic in L beyond that, and is weakly speed-dependent. The CLI default L = round(DPI/50.8) (`calibration/cli.ts:79-80`) exceeds 31 above ≈1600 DPI.
2. **Saturation carry (capped).** Output beyond ±32767 is kept in `rem` (up to ±2147418112 Q16.16 ≈ 32767 counts, i.e. about one extra report) and released over later reports — also after a profile change or while the curve is off. Anything beyond that cap is **dropped**: the independent verification fed `dx = 30000` through a 16× profile and about 414 k of 480 k counts were lost. ODD's policy is therefore *capped carry*, recorded in KineForge as `carry_capped`. forge-dirgain chose the opposite (drop). KineForge must pick one policy explicitly and record it in session metadata.
3. **Inconsistent clamp points** across adapters (before the engine in evdev/Zephyr; after it in QMK 8-bit). A KineForge simulation of "what firmware X does" must model the specific adapter, not just the engine.
4. **Two calibration analyses** that disagree, with a silent fallback to accelerated input in the browser one. Calibration bins by **target** direction while the engine keys on **motion** direction via the estimator.
5. **Thin cross-language coverage.** Golden vectors exist only for `compute_scale` (2592 cases, C vs TS BigInt). Smoothing, remainders, carry, 2×2, reverse reset, idle reset, sensor-DPI gating and HID responses are tested only in C; the TS mock engine is partial and never compared with C.
6. Smaller: Q8.8 resolution (1/256 ≈ 3.1 DPI steps at 800 CPI); two headroom normalisations (C integer vs TS float); `SIMULATE_DELTA` mutates live state; evdev adapter uses CLOCK_REALTIME and a fixed base of 800; `record-evdev` assumes the 64-bit `input_event` layout.
7. **Mirror asymmetry with L > 0** (floor shifts): with a point-symmetric profile, 100 k random steps differ in `S` on most reports (mean 0.0085 %, max 3.44 % on single reports; summed output 1085690 vs 1085633 counts, reproduced). Negligible in practice, but a port must replicate floor semantics exactly.

## 7. Portability and performance

- **Freestanding**: headers are only `<stdbool.h>`, `<stddef.h>`, `<stdint.h>` (`<string.h>` only when hosted). x86-64 `-ffreestanding -fno-builtin`: only `memset` undefined. Cortex-M4/M33 `-Os`: `.text` ≈ 2.78 KB, only `memset`. Cortex-M0+ additionally needs libgcc division/multiply helpers.
- **Implementation-defined behaviour a port must mirror** (not UB): arithmetic `>>` on negative `i64` (`.c:457`, `.c:550-551`, `.c:562-563`) floors — Rust's `i64 >> n` matches; `u16 → i16` wrap in `SIMULATE_DELTA` — Rust `as i16` matches; `u32` modular clock subtraction — `wrapping_sub`.
- **wasm32**: nothing in the source blocks it and the API passes only pointers and scalars (immune to the historical aggregate-ABI mismatch of `wasm32-unknown-unknown`), but compiling it requires clang with the wasm backend or wasi-sdk — **not verified here** (neither clang nor rustc is installed).
- **Cost**: 24.5 ns/report on x86-64 -O2 (scratch benchmark); estimated 150–300 cycles on Cortex-M4 (not measured); 64 B state.

## 8. Integration options for KineForge

| Option | For | Against |
|---|---|---|
| **A. C library + Rust FFI (`cc` crate)** | single source of truth | every browser build needs clang/wasi-sdk; `unsafe` boundary; two languages in every debug session |
| **B. Rust port, exact behaviour** | pure Rust everywhere incl. wasm and `no_std`/MCU; one-to-one operation mapping; overflow panics in debug builds act as a bug detector | a second implementation to keep in lock-step |
| **C. Language-neutral contract (spec + vectors + profile format)** | already half exists | not sufficient alone: stateful semantics are defined only by the C code |
| **D. Both, with conformance tests** | catches drift | needs the vectors C lacks today |

**Recommendation: B, governed by C + D.**

- `kf-dirgain`: `#![no_std]`, allocation-free, zero dependencies; targets native, `wasm32-unknown-unknown`, and a thumbv7em check build.
- The C engine remains the oracle (it ships in firmware; the spec says code + vectors are authoritative).
- No change to ODD is required for KineForge to start. Two **proposed** (not performed) contributions to ODD would strengthen the contract, to be raised with the maintainer (you): a stateful trace-vector generator, and fixes/decisions for issues 1–2.

**Bit-exact equivalence test plan**

1. **Existing fixtures**: all 2592 `scale_vectors.json` cases (deserialize → bytes round-trip → `(scale, octant)`); the 17 C unit tests with identical literal expectations; CRC check values; JSON ↔ bytes cases from OpenMouse tests 11/19; capability-byte assertions (`tests/test_directional_dpi.c:233-272, 524-536`); QMK framing test.
2. **New stateful trace vectors**, generated by a C program modelled on `tests/gen_vectors.c`: per profile, a sequence of `(dx, dy, now_ms[, sensor_dpi])` → `(out_dx, out_dy, rem_x, rem_y, dir_x, dir_y, last_octant, last_scale)` after every step, covering L ∈ {0, 1, 16, 31, 32, 64, 128}, every flag bit, the 2×2 model, reverse reset, saturation carry across steps (±32767, −32768), idle gaps at threshold −1/0/+1 including `u32` wrap, sensor-DPI switches, plus HID request → response byte pairs for every opcode and error path. Ideally committed in ODD so its CI regenerates and diffs them like the existing vectors.
3. **Differential fuzzer**: a dev-only, native-only `c-oracle` feature compiles `core/directional_dpi.c` via the `cc` crate and compares the full engine state after each of millions of seeded random steps; runs in Linux CI.
4. **Calibration**: if reused, freeze one analysis (the TS core, not the `calibrate.html` copy) with fixtures from real logs before porting.

## 9. Directly reusable components

- Spec documents, 28-byte profile + JSON schema + CRC; `scale_vectors.json` and the `gen_vectors.c` pattern.
- C core as oracle and as firmware code; the evdev filter as a host-side reference; the Zephyr filter as the "engine between sensor and HID" pattern.
- TS: `crc16Ccitt`, (de)serialise, `computeDirectionalScaleQ16_16` (BigInt-exact), JSON import/export, the WebHID client with receiver relay and QMK framing — directly usable by a browser UI if KineForge keeps a TS layer.
- `tools/odd_profile.py` (hidraw/hidapi device layer); MouseTester CSV parser; `record-evdev.ts`; KovaaK's kill-alignment logic.

## 10. Related directories (not ODD)

- `/home/lloen/src/forge-dirgain` — **not a git repository**; predecessor prototype with **no shared code**: C core with a 257-entry atan LUT and full-circle angle, an `angle_cdeg` rotation offset, no estimator, drops saturated motion, 4 × 34-byte profiles with its own protocol. Superseded by ODD. Ideas worth keeping: Q16.16 nodes and a sensor-rotation offset.
- `/home/lloen/src/mouse-firmware-re` (`github.com/0zLloen/mouse-firmware-re`) — reverse-engineering notes and flashing tools for the Glorious hardware; no engine code. Canonical for hardware facts; ODD is canonical for the algorithm.

## 11. Open questions for the maintainer

1. Is the L ≈ 31 effective cap intended? Options: clamp L to 31 in v1.x, rescale `d`, or widen the bound in v2. This decides whether KineForge can offer "window in mm" honestly.
2. Saturation policy for KineForge simulations: capped carry (ODD) or drop (forge-dirgain)? (Proposal: model ODD exactly — `carry_capped` — when simulating ODD firmware; make it a recorded parameter otherwise.)
3. Should stateful trace vectors live in the ODD repo as part of the spec?
4. Will KineForge's engine ever need to run on an MCU? (Strengthens `no_std`.)
5. Which calibration analysis is authoritative?
6. Is profile v2 (finer nodes, rotation offset) planned before KineForge freezes its import/export?
