# Open Directional DPI (ODD): technical audit for KineForge

Audit date 2026-11-13. Read-only. All experiments ran on a `git archive HEAD` copy and on harnesses under
`<scratch>/odd` and `<scratch>/exp` (scratch = `/tmp/claude-1000/-home-lloen--claude-spares-spare-2GQwMQ/95815f0b-46c2-4973-91bb-aaae9ed96bbc/scratchpad`).
Paths below are relative to `/home/lloen/src/open-glorious-one-firmware` unless stated. `file:N` means line N.

---

## 0. Identity, branches, upstream

- The checkout really is ODD. `origin` = `git@github.com:0zLloen/open-directional-dpi.git`. The directory name comes from the project's start as Glorious custom-firmware research (untracked, gitignored `PROGRESS.md`, dated Nov 5).
- Local refs:
  - `refactor/repo-layout` = `c177d23` (HEAD, clean tree). Equal to `origin/refactor/repo-layout`.
  - `main` = `a7f41f4`. This is the merge of PR #9.
  - `feat/shift-profiles-radio` = `e2dddf7`. It is not remote-only: it is a local branch checked out in a worktree at `/tmp/claude-1000/-home-lloen-src-open-glorious-one-firmware/.../wt/ddpi`, and it is also on origin. Its tree equals `main`'s tree (empty diff), so it is fully merged.
- `main..refactor/repo-layout` (one commit, 18 files) changes no code:
  - It moves `tests/test_qmk_raw_hid.c` to `adapters/qmk/tests/` with a new `adapters/qmk/Makefile`.
  - It moves `calibration/providers/builtin/calibrate.html` to `web/calibrate.html`.
  - It adds an `aimlabs` alias in `calibration/cli.ts:156`, adds the new `adapters/glorious-cm2856/README.md`, and rewrites the docs and CI paths.
  - `core/`, `spec/` and the vectors are unchanged.
- Upstream (checked with `git ls-remote` and the `gh api` compare; nothing was fetched):
  - `origin/main` is now `3b12333`, the merge of PR #10 (`refactor/repo-layout`). Compare `c177d23...3b12333`: ahead_by 1, behind_by 0, **0 files differ**.
  - So the working tree equals upstream `main`'s content. Local `main` is 1 merge commit behind because it was never fetched.
- History: initial commit `4df8a22` (2026-11-05). The core changed in `d0762df` (estimator wrap fix, atan constant, `process_motion`), `0474ba9` (`ONLY_AT_BASE_DPI`) and `21ea252` (clock in caps). The project is 8 days old and very actively edited.

## 1. Structure, languages, build, licence, CI, tests

| Area | Files (LOC) | Lang |
|---|---|---|
| Core engine | `core/directional_dpi.{h,c}` (311+827) | C11 (actually C99-clean) |
| Spec | `spec/directional-dpi-v1.md` (205), `spec/hid-protocol-v1.md` (181) | md |
| C tests/vectors | `tests/test_directional_dpi.c` (561, 17 tests), `tests/gen_vectors.c` (107), `tests/vectors/scale_vectors.json` (2647 lines, 2592 vectors) | C/JSON |
| TS mirror + WebHID client | `integrations/openmouse/src/directional-dpi.ts` (1076), `mock-hid.ts` (514), test (624, 21 suites) | TS (strip-types, no build) |
| Calibration | `calibration/{cli.ts, core/*, providers/*}` (~900), tests (656) + 2 CSV fixtures | TS |
| Browser calibrator | `web/calibrate.html` (593, self-contained JS, GitHub Pages) | HTML/JS |
| Device tool | `tools/odd_profile.py` (489) + test (466) | Python stdlib (+optional hidapi) |
| Adapters | `adapters/linux-evdev` (362), `adapters/qmk` (144+45+test 165), `adapters/zephyr` (291 + tests 416 + DT binding + Kconfig), `adapters/glorious-cm2856` (patch C 277, ld, `build_and_verify_patch.py` 697 Unicorn, `apply_patch.py`) | C/Python |

- **Build:** plain Makefiles (`tests/Makefile`, `adapters/*/Makefile`). Zephyr uses CMake/west (`zephyr/module.yml` → `adapters/zephyr`). TS has no build step: `node --experimental-strip-types`. `tsc` is only a type-check (`package.json`, `tsconfig.json`: strict, `noEmit`). The only devDeps are `typescript 5.9.3` and `@types/node`.
- **Licence:** MIT (`LICENSE`, SPDX headers everywhere).
- **CI** (`.github/workflows/ci.yml`):
  - A binary/vendor-file guard and `tsc`.
  - `make -C tests test`, then regenerate the vectors and require `git diff --exit-code -- tests/vectors`.
  - The evdev selftest and `make -C adapters/qmk check`.
  - QMK adapter compiled against the real QMK 0.34.8 headers.
  - The Glorious patch compile/link guard with `gcc-arm-none-eabi`: size, no `.bss`/`.data`, no libgcc helpers.
  - The OpenMouse TS suite, the calibration TS suite, the Python device-layer tests and the patch-file tests.
  - `zephyr.yml`: twister on `native_sim/native/64` for Zephyr v4.5.0 and v3.7.2, plus a ZMK `nice_nano` build.
  - `pages.yml`: tests the WebHID block, then publishes `web/calibrate.html`.
- **I re-ran in scratch:**
  - C 17/17 tests, also under ASan+UBSan with `-fno-sanitize-recover`.
  - Vectors regenerate byte-identical, also from the UBSan build.
  - OpenMouse 21/21, calibration 15/15, `test_odd_profile.py` OK, `test_apply_patch.py` OK, evdev selftest OK, QMK raw-HID test OK.
  - Not run: Zephyr/ZMK (no west/Zephyr) and the Unicorn patch verifier (no stock image, no `unicorn`).

## 2. Core algorithm (exact)

### 2.1 Number formats

- **Q8.8, u16.** Used for the multipliers `M[0..7]` and the global gain `G` (`.h:31-34`). The range is 16..1024 (0.0625x..4x). Clamping happens in `clamp_q8_8` (`.c:40-48`) on sanitize. **Deserialize rejects** out-of-range values instead of clamping them (`.c:226-240`).
- **Q16.16:**
  - The scale `S` is u32, max 16.0x = 1048576.
  - The remainders `rem_x`, `rem_y` are i32.
  - Intermediates are i64/u64.
- The fields `dir_x_q16_16`, `dir_y_q16_16` are **misnamed**: they hold counts×256 (Q8), bounded to |d| ≤ 16383 (`.c:538-571`).
- **No floats** anywhere in the core. I checked x86-64 `objdump` (integer SIMD only, 3 `div`) and ARM. `<math.h>` is used only in the tests.

### 2.2 Direction → octant (`ddpi_compute_scale_q16_16`, `.c:347-461`)

```
if profile==NULL || (dx,dy)==(0,0) || is_identity(profile): return (65536, octant 0)        # .c:353-358
ax=|dx|, ay=|dy| as u32 (−32768 → 32768)                                                     # .c:55-57,360-361
quadrant by signs (a zero component counts as ≥0), then octant o, card/diag indices, (u,v)=(max,min):  # .c:369-425
  dx≥0,dy≥0: ax>=ay → o0 (E,SE)  else o1 (S,SE)
  dx<0, dy≥0: ay>=ax → o2 (S,SW)  else o3 (W,SW)
  dx<0, dy<0: ax>=ay → o4 (W,NW)  else o5 (N,NW)
  dx≥0,dy<0: ay>=ax → o6 (N,NE)  else o7 (E,NE)
  (at |dx|==|dy| r=65536 so the result is the diagonal node whichever octant label wins)
```

Octants are HID-CW (+Y = toward the user): 0 E→SE, 1 S→SE, 2 S→SW, 3 W→SW, 4 W→NW, 5 N→NW, 6 N→NE, 7 E→NE.

### 2.3 Fraction across the octant (`.c:431-452`)

```
r = (v>=u && u>0) ? 65536 : (v>0 ? floor((v<<16)/u) : 0)            # one u32 UDIV; v<<16 ≤ 0x7FFF0000
t = r
if !(flags & LINEAR_OCTANT_RATIO) && 0<r<65536:
    t = min(65536, r + floor(22787 * r * (65536 - r) / 2^32))       # u64; (4/π)atan(x) ≈ x + 0.3477·x(1−x)
```

- I verified numerically with an exhaustive sweep of v ∈ [0, 32768] at u = 32768:
  - The max angle error is **−0.2150°** at r ≈ 42366 (32.9° into the octant).
  - The linear mode errs by up to 4.07°.
  - `t` is monotonic in `r`.
  - k = 0.3477 is the minimax coefficient for this form (best is 0.2144°).
- This matches spec §4 (`spec/directional-dpi-v1.md:73-80`).
- There is no LUT and no atan2. The bins are exactly 45° apart, with a fixed 0° origin and no rotation offset.

### 2.4 Interpolation and gain (`.c:454-460`, frame mapping `.c:338-345`)

```
M(k) = flags&FRAME_SENSOR_CCW ? mult[(8-k)&7] : mult[k]      # HID-CW index k
Mc = M(card)<<8 ; Md = M(diag)<<8                             # Q16.16, i32
Mdir = Mc + ((i64)(Md-Mc) * t >> 16)                          # ARITHMETIC shift: floor toward −∞ (impl-defined in C)
S = (u64)Mdir * G >> 8                                        # floor; u32 Q16.16
```

- The interpolation is linear in angle between neighbouring 45° nodes. The curve is C⁰ around the circle (shared boundary nodes); it is not C¹.

### 2.5 Travel-window direction estimator (`update_direction_estimator`, `.c:517-572`)

```
L = min(reserved&0xFF, 128); if L==0: est=(dx,dy)
s = |dx|+|dy| (L1); if s==0: est=(0,0) (unreachable: zero reports return earlier)
next = 256*Δ
if d != 0: α = floor((2L<<16)/(2L+s)) ; next += (i64)d*α >> 16      # floor shifts; 2nd UDIV
while |next_x|>16383 || |next_y|>16383: next >>= 1 (both, arithmetic)   # |·| tracked on a separate unsigned copy
d = next ; est = (i16)d
```

- **Documentation bug.** The comment at `.c:543-544` says "second-order Padé of exp(−s/L)". In fact α = 1/(1 + s/2L) is the [0/1] Padé of e^{−s/2L}. The spec (`spec:96`) has it right: 1/e per 2L counts.
- **Undocumented saturation (probe `<scratch>/exp/probe2.c`).**
  - The steady-state |d| ≈ 256·(2L+s). The 16383 bound therefore engages once 2L+s > 64.
  - Each halving shortens the memory relative to new input. As a result:
    - The effective window is faithful only for **L ≲ 31 at 1 count/report**, and for less at larger per-report steps.
    - L = 64 and L = 128 behave like an effective window of about 32–50 counts and are **non-monotonic** in L.
  - Measured counts of a 90° turn until the estimate passes 45°, at 1 count/report:

    | L | measured | ideal ln2·2L |
    |---|---|---|
    | 16 | 23 | 22 |
    | 32 | 44 | 44 |
    | 64 | 40 | 89 |
    | 128 | 43 | 177 |

  - `cli.ts:79-80` sets the default L = DPI/50.8, which exceeds 31 above ~1600 DPI. At 3200 or 6400 DPI the calibrated "0.5 mm" window is therefore not what the engine does.
  - It also makes the direction estimate weakly speed-dependent.
- **Asymmetry.** With L > 0, the floor shifts make mirrored input sequences differ:
  - A point-symmetric profile, 100k random steps: 95% of reports differ in `S` (mean 0.0085%, max 3.4% on single reports), and summed output differs by 57 out of ~1.09M counts.
  - With L = 0 it is exactly symmetric (0 mismatches in 200k).
  - Practically negligible, but **any port must replicate floor semantics** (Rust `i64 >> n` is arithmetic, which matches).

### 2.6 Per-report processing (`ddpi_process_delta`, `.c:574-647`; `ddpi_process_motion`, `.c:649-664`)

```
curve_off = is_identity(p) [ENABLED clear | BYPASS set | G==256 && all M==256]
            || (ONLY_AT_BASE_DPI && sensor_dpi!=0 && sensor_dpi!=base_dpi)            # .c:312-317
if curve_off: out = in with −32768→−32767; state untouched (rem & d persist!)          # .c:598-603
if Δ==(0,0): out 0, no state change                                                    # .c:605-609
if LINEAR_2X2:  gx=M(E)·G, gy=M(N)·G, h=trunc((M(NE)·G − M(NW)·G)/2)   (all Q16.16 i32)
                sx = dx·gx − dy·h ; sy = dy·gy − dx·h  (i64); last_scale=(gx+gy)/2; octant not updated  # .c:614-632
else:           est = estimator(Δ); S = scale(est); sx = dx·S; sy = dy·S                 # .c:634-646
emit per axis (emit_from_q16_with_remainder, .c:482-511):
  if scaled==0 && raw==0: return 0 (rem kept)
  if RESET_REMAINDER_ON_REVERSE && sign(raw) opposes sign(rem): rem=0
  total = scaled + rem (i64)
  out = trunc(total/65536) (toward 0, via magnitude shift .c:463-470); clamp ±32767
  rem = clamp(total − out·65536, ±2147418112)        # excess beyond the clamp is CARRIED, not dropped
process_motion: if Δ≠0 and (u32)(now − last_motion) ≥ idle_ms → d=(0,0); last_motion=now; then process_delta.
idle_ms = (reserved>>8)·5, or 50 if 0.
```

- `KEEP_REMAINDER_ON_REVERSE` (bit 1) is a no-op.
- The 2×2 model ignores `M[S]`, `M[W]`, `M[SE]` and `M[SW]`.
- A profile change (`set_profile`) resets the state only if the new profile is an identity (`.c:327-336`). A saturation carry, which can be up to 32767 counts, survives profile changes and curve-off periods and comes out later.
- `ddpi_engine_set_sensor_dpi` clears `rem` and `d` on any change (`.c:295-301`).

### 2.7 Other helpers

- **CRC.** CRC-16/CCITT-FALSE: poly 0x1021, init 0xFFFF, no reflection, no xorout (`.c:63-76`).
- **`normalize_cpi_headroom`** (`.c:125-164`):
  - max_eff = max(M·G) in Q16.
  - sensor = round(base·max_eff / 65536) in u64, clamped to [50, 65535].
  - M'ᵢ = clamp(round(Mᵢ·G·256 / max_eff), ≤ 256), and G' = 256.
  - The calibration TS path does its own float version (`profile-generation:93-99`). The two are not bit-equal and no test compares them.
- **`set_ccw_frame`** swaps i ↔ 8−i for i = 1..3 and toggles bit 3 (`.c:166-187`).

## 3. Data structures, profile and protocol

- **28-byte profile v1** (`.h:114-123`, spec §7 `spec:134-178`). All little-endian, explicit byte packing (`.c:17-38`), no type punning:

  | Offset | Field |
  |---|---|
  | +0 | magic 0x4450 (bytes `50 44`) |
  | +2 | version 1 |
  | +3 | flags |
  | +4 | base_dpi (≥ 50) |
  | +6 | G |
  | +8..+23 | M[8] |
  | +24 | reserved (low byte = L, high byte = idle in 5 ms units) |
  | +26 | CRC over bytes 0..25 |

- The flags byte is not validated; `BYPASS` (bit 6) could be persisted.
- **There is no spare space**: `reserved` is fully used, so any v2 needs a version bump.
- **In-memory layout:** identical on x86-64, i386 (ILP32, like wasm32) and ARM Cortex-M4. I checked this with `offsetof` in `<scratch>/exp/layout.c`:
  - profile: 28 B, no padding.
  - `ddpi_engine_t`: 64 B, align 4. `rem_x`@28, `dir_x`@36, `last_octant`@44 (3 pad), `last_scale`@48, `last_motion`@52, `sensor_dpi`@56, `clock_known`@58, `clock_ms`@60.
  - There is no `int64`, pointer or `long` in any struct.
- **JSON `open-directional-dpi-v1`** (spec §8 `spec:182-199`):
  - Writers: TS `exportProfileJson`/`importProfileJson` (`directional-dpi.ts:477-593`) and Python `encode`/`decode` (`tools/odd_profile.py:83-166`).
  - Q8.8 = round-half-up(x·256): `Math.round` in TS, `Decimal ROUND_HALF_UP` in Python.
  - Floats are written with `toFixed(4)`, which round-trips exactly.
  - `directionalMultipliers` keys are always physical HID directions (`E_0` … `NE_315`); `frame` only decides the storage order.
- **HID protocol v1** (`spec/hid-protocol-v1.md`; handler `.c:666-827`):
  - One 64-byte unnumbered Feature report on usage page 0xFFFF/0x0000.
  - Packet: `'D','P', ver, status, seq, 0xDF, opcode, slot, payload[54], CRC16 LE over bytes 0..61`.
  - Handler gating: non-ODD packets (wrong magic or namespace) return `false` untouched. A wrong version or CRC answers 0xA2.
  - Opcodes: 01 GET_CAPABILITIES, 02 GET, 03 SET, 04 SAVE_FLASH, 05 RESET (keeps `base_dpi`, `sensor_dpi` and clock), 06 SET_ENABLED (0 off, 2 bypass, other on), 07 SIMULATE_DELTA, 0F ENTER_BOOTLOADER with magic "BLRS".
  - **SIMULATE_DELTA** runs on the **live** engine via `process_delta`, with no idle reset. It returns out, rem, octant and scale.
  - **GET_CAPABILITIES** payload (`.c:707-731`, spec §4.1):
    - "DP01" magic, engine ver, max_slots = 1, active_slot.
    - Flags: b1 save, b3 bootloader, b4 window and b5 2×2 always set, b6 sensor_dpi known, b7 clock valid.
    - min/max DPI 50/65535, min/max multiplier 16/1024, window default/max 16/128, `sensor_dpi` u16, `clock_ms` u32.
  - **Transports:**
    - Glorious: Feature report, or the 2.4 GHz receiver relay envelope (`body[2] = 0x02`, `body[3] = 0x3A`; the CRC covers the canonical bytes; link query 0x89; seq ≥ 5 to stay clear of stock handlers).
    - QMK: 6 lock-step 32-byte raw-HID frames, `[0xDF, chunk, 30 data bytes]` (§6).
    - Zephyr: the board's own HID handler calls `odd_input_hid_feature_report`.
  - Device identification: product string `[OpenDDPIv1]`, the `DP01` stamp in the stock version reply, or the QMK probe.

## 4. Calibration

- **What it measures:** for each flick, the "primary ballistic submovement" endpoint, as a radial ratio = (endpoint projected on the target direction) / (target distance).
  - Ratio > 1 means overshoot. The heading error is also computed but unused.
  - Core function: `calibration/core/trajectory-analysis/index.ts:70-158`.
  - Speed = per-sample step / max(dt, 0.125 ms).
  - Endpoint = the first sample after peak speed with speed ≤ 0.18·peak and projection ≥ 0.4·target.
  - Submovements are counted with a 0.20/0.22 hysteresis.
  - These thresholds are heuristic and come with no citation or validation data.
- **Fitting** (`directional-error:54-95`, `profile-generation:51-131`):
  - Bin by the nearest CCW 45° sector of the **target** direction.
  - Keep ratios in [0.4, 2.2].
  - Median plus MAD rejection: |r − med| ≤ max(0.15, 3·1.4826·MAD).
  - Gain = clamp(1/median, 0.8, 1.25), then a circular 3-tap smoothing [0.15, 0.70, 0.15].
  - Normalization: mean-preserving (CLI default), cpi-headroom (the library default!) or none.
  - There is no regression or optimiser, and no compensation for the engine's own interpolation between bins or for systematic human undershoot (the mean-preserving step only removes the global bias).
- **Input sources:**
  - **`mouse-log` / `aimlabs`** (`providers/mouse-log/index.ts`):
    - Reads MouseTester CSV: line 3 is the header `xCount,yCount,Time (ms),buttonflags`; rows hold dx, −dy (MouseTester negates Y), ms, and RAWMOUSE flags.
    - A flick runs from one left-click to the next; the target is the displacement between clicks.
    - Samples are resampled to a 4 ms zero-order hold and kept for 100 ms after the click.
    - Flicks longer than 1.5 s or shorter than 0.3 × the median length are dropped.
  - **Logs come from** MouseTester (Windows Raw Input) or `providers/mouse-log/record-evdev.ts` (Linux evdev, not grabbed).
  - **`kovaaks`** (`providers/kovaaks/index.ts`):
    - Uses the mouse log plus `Stats.csv` kill timestamps.
    - The clock offset is found by brute-force alignment with a ±30 ms tolerance.
    - Only flicks from kill to kill are kept.
  - **`builtin`** = `web/calibrate.html`:
    - A center-out task: 8 directions × {140, 210, 280} counts, Fisher–Yates shuffled.
    - Input is **Pointer Lock `movementX/Y`** with `{unadjustedMovement: true}`. It **silently falls back** to OS-accelerated movement if that is rejected (`calibrate.html:456-459`).
    - Events come from `pointerrawupdate` when present, else `mousemove`, with timestamps from `e.timeStamp` and samples at least 4 ms apart.
    - **It has its own, different copy of the analysis** (`calibrate.html:160-171, 389-408`): dt ≥ 1 ms, Euclidean rather than projected distance, a 45% gate, ratio in (0.4, 2.0) exclusive, and no MAD.
    - So the page and the CLI pipeline are not equivalent, and nothing tests that they agree.
    - The page itself sends the profile over WebHID.
- **Outputs** (`cli.ts:121-151`):
  - JSON (`odd-sensor-ccw` frame). `baseDpi` = the play DPI, or the raised DPI for cpi-headroom. `windowCounts` = DPI/50.8 by default.
  - `evdev`: 8 float multipliers with G folded in as `m·G/65536`, plus the window. The filter rounds them back to Q8.8, which is lossy when G ≠ 1.
  - `qmk`: C lines for `keymap.c`.
  - `dts`: devicetree properties.
  - evdev and dts refuse 2×2, `onlyAtBaseDpi`, reverse-reset and the linear ratio. evdev also refuses non-default idle.
- **Status:** `mouse-log` and `kovaaks` have never run on a real session; the tests use synthetic logs (`calibration/README.md:13`). The synthetic recovery test accepts a systematic ×0.985 bias (`providers.test.ts:103`).

## 5. Adapters: input semantics, timing, polling

| Route | Input to the engine | Clock for idle reset | DPI / sensor_dpi | Output | Notes |
|---|---|---|---|---|---|
| Glorious CM2856 patch (`adapters/glorious-cm2856/stock_ddpi_patch.c:217-245`) | **Raw PAW3370 burst counts** (int16 at stock `0x2000302E`/`30`), after the sensor read and before the USB/RF report | Count of burst calls, ~1 per 1 ms TIMER1 tick (measured 1000.0/s on cable; RF inferred) | Hook in the stock sensor DPI writer `0x17860` (`:254-258`) | Writes back into the stock dx/dy | `PATCH_ROM` 0x1300 = 4864 B (`.ld:13`); the local untracked build ELF has `.text` 3464 B (~1.4 KB headroom); state at SRAM `0x2003F000`; profile in flash page `0x7E000`; Unicorn-verified, partly hardware-verified (§5 of hid spec) |
| QMK (`adapters/qmk/odd_pointing_device.c:82-102`) | `report_mouse_t` x/y after QMK's sensor driver, in `pointing_device_task` | `timer_read32()` ms | `pointing_device_get_cpi()` every task | `CONSTRAIN_HID_XY` **after** the engine: in 8-bit mode the excess above 127 is lost, not carried (probe: 2× gain at 100 counts/report emitted 12700 of 20000) | Raw-HID transport `:112-143`; EEPROM save; never built in real QMK or run on a device |
| Zephyr / ZMK (`adapters/zephyr/src/odd_input_filter.c`) | Zephyr input `REL_X/REL_Y` events, accumulated per sync frame and clamped to ±32767 **before** the engine (`:120`) | `k_uptime_get_32()` | Not reported (no `set_sensor_dpi`, so `ONLY_AT_BASE_DPI` is inert; caps 0x30) | Re-emitted on the callbacks of a virtual device; Y always carries sync | `merge-axis-syncs` for sources that sync per axis; spinlock around the engine; settings save deferred 500 ms; tested on native_sim |
| Linux evdev → uinput (`adapters/linux-evdev/odd_evdev_filter.c`) | Kernel `REL_X/REL_Y` per `SYN_REPORT` (USB HID counts, before libinput); the frame is clamped to ±32767 before the engine (`:82`), so the excess is dropped | **Event timestamps**, default CLOCK_REALTIME (no `EVIOCSCLOCKID`), truncated to u32 ms (`:79`) | Fixed base 800, no G or idle args (`:209-230`) | uinput clone of the device identity, grabs the source | `SYN_DROPPED` handling and button-state replay |
| Windows/macOS | No runtime filter. The "loader" = `apply_patch.py` (applies `OpenDDPIv1-patch.json` to the user's stock hex) + `odd_profile.py --backend hidapi` + `calibrate.html` WebHID | — | — | — | All untested on those OSes (`adapters/glorious-cm2856/README.md:107-109`) |
| OpenMouse (`integrations/openmouse/src/directional-dpi.ts`) | Host editor/client only: a WebHID client, the `compute_scale` mirror and a polar-curve preview | — | Reads caps | — | Not merged into the OpenMouse app; tested only against `mock-hid.ts` |

## 6. Mouse logging

- The only recorder in-repo is `calibration/providers/mouse-log/record-evdev.ts`:
  - It reads the 24-byte `struct input_event` (64-bit time layout only: wrong on 32-bit userspace) and sums REL_X/REL_Y per `SYN_REPORT`.
  - It writes one MouseTester row per non-empty frame: `dx,-dy,ms(3 decimals = µs resolution, relative to the first event),flags`.
  - The time source is evdev event time, CLOCK_REALTIME by default and non-monotonic in principle.
  - It does not handle `SYN_DROPPED`, records only the left button, and does not grab the device.
- MouseTester (an external Windows tool) produces the same CSV with ms timestamps.
- `calibrate.html` keeps traces only in memory and exports only the profile.
- There are no device-side timestamps and no per-report HID logging. `SIMULATE_DELTA` can read back engine state.

## 7. Tests and validation, and what is not covered

- **C** (`tests/test_directional_dpi.c`, 17 asserts-based tests). They cover:
  - identity passthrough, the 8 control points and 360° continuity;
  - G, remainder accumulation, saturation carry (20000@2x → 32767 + 7233 carry → next (1, −1) = 7235);
  - serialize/CRC, the HID handler, chunking invariance (1 kHz vs 8 kHz within 3 counts);
  - the CCW frame, headroom, 2×2/bypass, the large first sample, idle reset, null callbacks, high CPI and the single DPI stage.
- **Golden vectors:** `tests/vectors/scale_vectors.json`.
  - 8 serialized profiles × 324 (dx, dy) = 2592 entries of `compute_scale` plus octant.
  - The profiles cover identity, disabled, HID-CW, CCW, linear ratio, G = 451, the 16/1024 extremes at 45000 DPI, and bypass.
  - The grid is ±{0, 1, 99, 100, 414, 1000, 12345, 32766, 32767} plus −32768.
  - **Cross-language check:** TS `computeDirectionalScaleQ16_16` (BigInt math) matches all of them, and the TS serializer reproduces the C bytes (`openmouse test 7`).
- **TS ↔ Python:** the JSON ↔ 28-byte mapping, byte-exact (test 11: 2048 flag/window/idle combinations, TS only; test 19: 3 docs through `odd_profile.py encode/decode`). Test 19 writes a temp file next to `tools/odd_profile.py` while it runs.
- **WebHID bytes:** `calibrate.html`'s WebHID block equals the TS client's bytes (`builtin-webhid.test.ts`, which evaluates the inline JS between the markers).
- **Hardware parity:** `RESET` + `SET_PROFILE` + `SIMULATE_DELTA` on a retail Glorious matched the C reference bit for bit (manual; `hid-protocol-v1.md:89`).
- **Gaps:**
  - **No golden vectors for any stateful path**: the estimator, remainders, saturation carry, 2×2, reverse reset, idle reset, sensor-DPI gating or HID responses. Only the C engine implements these; the TS mock (`mock-hid.ts:291-326`) is a partial port (no estimator, no 2×2), marked `ponytail:` and never compared with C.
  - Python has no engine.
  - The two calibration analysis implementations (`calibrate.html` vs the TS core) and the two headroom normalizations (C vs TS) are untested against each other.
- **My additional checks** (scratch):
  - UBSan/ASan fuzz of 4M `process_motion` calls (random profiles, all flags, extreme deltas, u32 clock wrap): no traps and never −32768 emitted.
  - `-Wconversion -Wsign-conversion -Wshadow -Wcast-align` etc. are clean.

## 8. Portability

- **Freestanding.** The headers are only `<stdbool.h>`, `<stddef.h>` and `<stdint.h>`. `<string.h>` is included only when `__STDC_HOSTED__` (`.c:9-14`); otherwise `memcpy`/`memset` are declared extern.
  - x86-64 `-ffreestanding -fno-builtin -O2`: the only undefined symbol is `memset`.
  - Cortex-M4 and M33 `-Os`: `memset` only, `.text` 2780/2772 B.
  - Cortex-M0+: also `memcpy`, `__aeabi_uidiv`, `__aeabi_idiv`, `__aeabi_lmul` and `__gnu_thumb1_case_uhi`. It would fail a Glorious-style `-nostdlib` link without libgcc.
  - No malloc and no stdio. No globals in the core: all state is in a caller-owned `ddpi_engine_t`, so it is reentrant per instance. There are no floats.
- **C standard.** It compiles cleanly as `-std=c11` and as `-std=c99 -pedantic`. C89 fails (`inline`). No C11-only features are used; `_Static_assert` appears only in the Glorious patch.
- **Endianness.** All wire formats are packed byte by byte as LE, so the code is endian-agnostic. wasm32 is LE anyway.
- **Implementation-defined behaviour (not UB) that a bit-exact port must mirror:**
  - Arithmetic `>>` on negative i64 at `.c:457`, `.c:550-551` and `.c:562-563`. It floors; gcc and clang do this, and Rust `>>` on i64 matches.
  - u16 → i16 conversion in `SIMULATE_DELTA` (`.c:792-793`); this matches Rust `as i16`.
  - I did not find any signed overflow or out-of-range shift: all products are widened to 64 bits, and `v<<16` stays below 2³¹.
- **wasm32 (not verified: no clang/wasi-sdk/rustc here).**
  - Nothing in the source blocks it: 64-bit integer math is native in wasm, and the struct layouts match ILP32 (verified on i386).
  - The public API is pointers and scalars only, with **no struct by value**. That makes it immune to the historic wasm32-unknown-unknown Rust C-ABI mismatch for aggregates.
  - What is required: clang with the wasm32 backend for the cc crate, and `memset` (and probably `memcpy` for struct copies) resolved by Rust's compiler-builtins or by bulk-memory. I could not compile or link for wasm.

## 9. Performance

- **Hot path per report** (angular model, L > 0):
  - 2 × 32-bit UDIV (r and α).
  - About 16 mul-class instructions (SMULL/UMULL/MLA, single-cycle on M4), counted from the `-Os` Cortex-M4 disassembly.
  - An octant branch tree.
  - A halving loop: 0–2 iterations typical, about 9 worst case on the first sample after idle with |Δ| ≈ 32767.
- **Code size:** `ddpi_process_delta` 580 B, `compute_scale` 316 B, `emit` 180 B, HID handler 614 B.
- **Estimated cost (not measured; no ARM emulator available):** ~150–300 cycles. That is 2–5 µs at 64 MHz (nRF52840), or 0.2–0.5% of a 1 kHz frame and ~2–4% at 8 kHz.
- **Host:** 24.5 ns/report on x86-64 -O2 (`<scratch>/exp/bench.c`).
- **RAM:** 64 B engine state.
- **Flash:** the Glorious patch budget (4864 B including the patch glue) is the tightest constraint. The engine is ~2.8 KB.

## 10. KineForge integration options

**(A) Keep the C library and call it through Rust FFI (cc crate).**

- Native: easy.
- Browser (wasm32-unknown-unknown):
  - Every developer and CI job needs clang with the wasm target, or wasi-sdk. gcc cannot do it, and none of these is installed here.
  - `memset`/`memcpy` must come from Rust's compiler-builtins.
  - There are two languages in every debug session and `unsafe` at the boundary.
- The ABI risk is low because there are no aggregates by value, but toolchain friction is high.
- It does keep one source of truth.

**(B) Port to Rust with exact behaviour preservation.**

- About 350 lines of real logic: CRC, serialize/deserialize/sanitize, `compute_scale`, the estimator, emit, 2×2, `process_motion`, and optionally the HID handler.
- Every operation maps one-to-one:
  - i64 `>>` is a floor;
  - integer `/` truncates toward zero, as in C;
  - `as i16` wraps, as in gcc;
  - `u32::wrapping_sub` gives the idle check.
- Use explicit `wrapping_*` only where C relies on modular u32. The fuzz found no overflow, so debug-build overflow panics would also act as a bug detector.
- A `#![no_std]`, alloc-free crate builds for native, wasm32 and even thumbv7em, so KineForge could later run the same code on an MCU.

**(C) Language-neutral contract: spec + golden vectors + profile format.**

- About half of it already exists: the spec, the 2592 scale vectors and the JSON/28-byte format.
- It is necessary under any option but not sufficient: the stateful semantics are covered only by the C implementation.

**(D) Maintain both, with conformance tests.**

**Recommendation: B, governed by C+D.**

- The C engine stays the oracle: it is what ships in firmware, and the spec says the code is authoritative (`spec:3`).
- KineForge gets a pure-Rust `no_std` port, so no C toolchain is ever needed for wasm.
- Equivalence is enforced by golden vectors that both repos consume, plus a native-only differential fuzzer.

**How to test bit-exactness:**

1. **Seed with the existing fixtures:**
   - All 2592 `scale_vectors.json` entries: deserialize the bytes, check that the bytes round-trip, and check `(scale, octant)`.
   - The 17 C unit tests ported with the same literal expectations.
   - CRC check values.
   - The OpenMouse test 11/19 JSON ↔ bytes cases.
   - The HID caps byte assertions (`test_directional_dpi.c:233-272, 524-536`).
   - The QMK framing test (`adapters/qmk/tests/test_qmk_raw_hid.c`).
2. **Add stateful "trace vectors"** from a new C generator modelled on `tests/gen_vectors.c`, committed in the ODD repo and regenerated and diffed in its CI the same way.
   - Format: per profile, a sequence of `(dx, dy, now_ms[, sensor_dpi])` → `(out_dx, out_dy, rem_x, rem_y, dir_x, dir_y, last_octant, last_scale)` after every step.
   - Coverage:
     - L ∈ {0, 1, 16, 31, 32, 64, 128}, to pin the cap behaviour;
     - every flag bit (1–7), the 2×2 model and the reverse reset;
     - saturation and carry across steps, ±32767 and −32768;
     - idle gaps at threshold−1, threshold and threshold+1 including u32 wrap, and sensor-DPI switches.
   - Also HID request → response byte pairs for every opcode and error path.
3. **Differential fuzzer:** a Rust dev-only `c-oracle` feature compiles `core/directional_dpi.c` with the cc crate (native only), runs millions of seeded random steps and compares the full engine struct after each one. Run it in CI on Linux.
4. **Calibration side:** if KineForge reuses the calibration analysis, first pick one implementation (the TS core, not `calibrate.html`'s copy) and freeze it with fixtures built from real logs.

## 11. Reusable components and fragile spots

**Reusable directly:**

- The spec documents.
- The 28-byte profile and JSON schema, and the CRC.
- `scale_vectors.json` and `gen_vectors.c` as a pattern.
- The C core, as oracle and as firmware code.
- The TS `crc16Ccitt`/serialize/deserialize/`computeDirectionalScaleQ16_16`/import/export (BigInt-exact) and the WebHID client with receiver relay and QMK framing.
- `odd_profile.py` (hidraw/hidapi device layer).
- The MouseTester CSV parser, `record-evdev.ts` and the KovaaK's kill-alignment code.
- The evdev filter as a host-side reference integration.
- The Zephyr filter, as a pattern for "engine between sensor and HID".

**Fragile or undocumented:**

1. The estimator's window cap (|d| ≤ 16383): L > ~31 is ineffective and non-monotonic, and the CLI's DPI/50.8 default exceeds it.
2. Misleading comment at `.c:543` ("second-order Padé, exp(−s/L)") and misnamed `dir_*_q16_16` fields.
3. The saturation carry can emit thousands of deferred counts after a flick, even after a profile change or curve-off period (only identity loads, `SET_ENABLED` 0/2 and DPI changes clear it). The sibling forge-dirgain deliberately chose the opposite: discard on saturation.
4. Adapters clamp before the engine (evdev, Zephyr: excess dropped) or after it (QMK 8-bit: excess lost). These clamp points are inconsistent across adapters.
5. Two diverging calibration analyses. `calibrate.html` silently falls back to accelerated pointer lock. The calibration targets are binned by target direction, while the engine is keyed by motion direction through the estimator.
6. Q8.8 resolution: 1/256 = 3.1 DPI steps at 800, ~74 at 19000. There is no space left in v1 for finer nodes.
7. Two headroom normalizations (C integer vs TS float).
8. `SIMULATE_DELTA` mutates live state.
9. evdev uses CLOCK_REALTIME timestamps, a fixed 800 base and float re-rounding of folded G.
10. `record-evdev` assumes the 64-bit `input_event` layout.
11. Many paths are labelled untested on hardware or OS: QMK on a device, Windows/macOS, WebHID on hardware, real-trainer logs, RF clock rate.

## 12. Related repos

- **`/home/lloen/src/forge-dirgain`:**
  - It is **not a git repo**: files dated 2026-11-05, plus a `docs/DIRECTIONAL-DPI-FIRMWARE-REVIEW.md`. It is the predecessor prototype and shares **no code** with ODD.
  - The canonical core there is C (`firmware/src/dirgain.c`, about 90 lines; plus `dg_protocol.c`).
  - TS (`upstream/mouse-protocol/src/directional-gain/`) is only a codec/client and the OpenMouse UI; it has **no TS gain math**. This differs from the brief's "TS core + C parity port".
  - `tests/simulation.c` is a 1 kHz × 1000 s host workload and protocol bridge.
  - **Different math from ODD:**
    - a 257-entry atan LUT with a full-circle angle, linear in angle;
    - an `angle_cdeg` rotation offset;
    - no direction estimator;
    - saturation **drops** the excess and the fraction;
    - bypass clears the fractions;
    - gain 0 is allowed;
    - 4 named profiles of 34 B each, with its own protocol.
  - It is superseded by ODD.
  - Ideas worth carrying to KineForge: the Q16.16 node proposal and the angle-offset (sensor rotation) concept.
- **`/home/lloen/src/mouse-firmware-re`** (`github.com/0zLloen/mouse-firmware-re`, HEAD `ec338ef`):
  - Contents: RE notes, the Compx bootloader protocol and USB flasher, `compx_hex_tool.py`, udev rules, and receiver/radio scripts.
  - It contains no engine code.
  - ODD depends on it only for the Glorious flash/patch tooling (`COMPX_TOOLS`) and its documentation references.
  - It is canonical for hardware facts, and ODD is canonical for the algorithm.

## Open questions

1. Is the L cap intended? Should v1.x clamp L to about 31, rescale, or change the bound? This affects any "window in mm" model in KineForge.
2. Which saturation policy does KineForge want: carry (ODD) or drop (forge-dirgain)?
3. Should the trace-vector generator live in the ODD repo, so the vectors become part of the spec?
4. Will KineForge ever need to run the engine on an MCU? If so, that is a further argument for B as `no_std`.
5. Which calibration analysis is authoritative?
6. Is a profile v2 with finer nodes or a rotation offset planned before KineForge freezes its format?
