# kf-dirgain-oracle

Native-only dev tool (not published, not for wasm or MCUs): the Open Directional DPI C
reference engine, vendored in `c/` (origin, commit and hashes in `c/SOURCE.md`), compiled
with the `cc` crate and used as the oracle for the Rust port `kf-dirgain`
(ROADMAP.md M2, OPEN_DIRECTIONAL_DPI_AUDIT.md §8).

- `src/c.rs` — `#[repr(C)]` mirrors of `ddpi_profile_v1_t` / `ddpi_engine_t` (a test checks
  size and every offset against the C compiler via `c/kf_shim.c`), the `extern "C"`
  declarations, and `COracle`, a safe wrapper whose state converts to and from
  `kf_dirgain::Engine` field by field. The only module allowed to use `unsafe`.
- `src/lib.rs` — `Op` (one engine operation, runnable on either implementation) and
  `XorShift` (seeded generator of profiles, deltas, timestamps, operations, HID packets).
- `src/bin/gen-traces.rs` — records `crates/kf-dirgain/tests/vectors/traces.json`.
- `tests/fuzz.rs` — differential fuzzer.

## Commands

```sh
# Regenerate the trace vectors (deterministic: no clock, fixed seeds), then replay them.
cargo run -p kf-dirgain-oracle --bin gen-traces -- crates/kf-dirgain/tests/vectors/traces.json
cargo test -p kf-dirgain --test traces

# Default fuzz: 2·10^6 compared engine ops + 2·10^5 profile buffers (~6 s in debug).
cargo test -p kf-dirgain-oracle

# Long fuzz: 10^8 compared engine ops + 10^7 profile buffers (release mode).
cargo test --release -p kf-dirgain-oracle --test fuzz -- --ignored
```

CI (`portable` job in `.github/workflows/ci.yml`) runs the long fuzz in release mode,
regenerates `traces.json` and fails if it differs from the committed file, and replays the
kf-dirgain tests on wasm32-wasip1 (`ci/test-wasi.sh`); the `native` jobs run the default
fuzz with the rest of `cargo test --workspace`.

Regenerate after re-vendoring the C (see `c/SOURCE.md`) or changing a scenario; the
generator prints steps per category. A fuzz failure prints the seed, the operation index,
the operation, the state before it and the differing fields; `engine_case(seed, 1000)` in
`tests/fuzz.rs` replays that case alone.

The fuzzer starts each case from a random base CPI and profile, then applies random
operations to both engines and compares the outputs and the full engine state after every
one: `process_motion` (timestamps a few ms apart, exactly at idle − 1 / idle / idle + 1, or
anywhere incl. the u32 wrap), `process_delta` (deltas biased to ±32767, −32768, octant
and clamp edges), `set_sensor_dpi`, `set_clock_ms`, `set_profile` (all flag bytes,
Q8.8 values 16..1024 incl. both extremes and out-of-range values, base CPI 0/49/50/65535,
random window/idle bytes), `notify_idle`, `reset_remainders`, `normalize_cpi_headroom` /
`set_ccw_frame` / `set_window_counts` on the live profile, HID requests (mostly
well-formed per opcode, also random payloads, unknown opcodes, broken framing, wrong
version, bad CRC; each callback absent / returning true / false), and occasional random
overwrites of remainders, direction and last-motion time. It also writes unsanitised
profiles straight into the live engine (`Engine::profile` is a pub field, `engine->profile`
a plain C struct field): any flags (2×2 forced on half the time), magic, version, base CPI
and window/idle bytes, and M and G in 0..=32767, so the 2×2 shear and `last_scale` and
`normalize_cpi_headroom`'s wrapping u32 arithmetic (M·G·256 wraps for M·G ≥ 2^24) are
compared beyond the sanitised range. The bound keeps the C defined: every M·G < 2^30, so
its int32 `gx + gy` and `gne − gnw` cannot overflow (a 65535 bound trips UBSan there).
Overwrites and raw writes are not counted; every counted op is compared.

The profile test sends random, serialized and re-signed 28-byte buffers (wrong magic and
wrong version drawn independently) through `deserialize`, `serialize`,
`sanitize_and_sign`, `normalize_cpi_headroom`, `set_ccw_frame`, `set_window_counts`,
`window_counts`, `idle_ms`, `is_identity`, `new_default`, `compute_scale_q16_16` and
`crc16_ccitt`.

## `traces.json` format

Header: `suite`, `generator`, `reference` (C file and commit), `response_fill` (238 = 0xEE),
`trace_count`, `step_count`, `traces`. Each trace:

```json
{"name": "idle/50ms", "base_dpi": 800, "profile_hex": "5044…" | null, "init": {state},
 "steps": [{"op": "motion", "dx": 10, "dy": 0, "now": 1000, "expect": {outputs…, state…}}, …]}
```

Replay: `Engine::new(base_dpi)`, then `set_profile(profile_hex)` unless it is null; the
state must equal `init`. Then for each step run the operation; `expect` holds its outputs
plus the full engine state afterwards, every field compared exactly.

Profiles (`profile_hex`, `saved_profile_hex`) are the 28 bytes of the fields **as stored**,
little-endian, with the stored `crc16`: no sanitising and no CRC check, so out-of-range
profiles can be passed to `set_profile`.

State keys: `rem_x`, `rem_y`, `dir_x`, `dir_y` (the `*_q16_16` fields), `last_octant`,
`last_scale`, `last_motion_ms`, `sensor_dpi`, `clock_known`, `clock_ms`, `profile_hex`.

| `op` | Arguments | Call | Extra `expect` keys |
|---|---|---|---|
| `delta` | `dx`, `dy` | `process_delta` | `out_dx`, `out_dy` |
| `motion` | `dx`, `dy`, `now` | `process_motion` | `out_dx`, `out_dy` |
| `sensor_dpi` | `dpi` | `set_sensor_dpi` | |
| `clock` | `now` | `set_clock_ms` | |
| `set_profile` | `profile_hex` | `set_profile` | |
| `notify_idle` | | `notify_idle` | |
| `reset_remainders` | | `reset_remainders` | |
| `normalize` | | `engine.profile.normalize_cpi_headroom()` | `sensor_cpi` |
| `ccw` | `use_ccw` | `engine.profile.set_ccw_frame(use_ccw)` | |
| `window` | `counts` | `engine.profile.set_window_counts(counts)` | |
| `hid` | `request` (64-byte hex), `save` (null = no callback, else its return value), `boot` (bootloader callback present) | `handle_feature_report` with a response buffer pre-filled with `response_fill` | `handled`, `response`, `save_calls`, `saved_profile_hex` (null if not called), `boot_calls` |

Coverage by trace-name prefix: `window/` (L = 0, 1, 16, 31, 32, 64, 128 and stored
129/200/255; 1-count and larger steps, 90° turns, staircase, the |d| ≤ 16383 clamp);
`flags/` (each bit alone, each with ENABLED, ten combinations incl. 0xFF); `2x2/` (shear
both signs, CCW frame, odd shear truncation, extremes with reverse reset, `cancel*`: a
scaled X of exactly 0 from a non-zero raw dx, with and without reverse reset); `reverse/`
(keep, legacy KEEP flag, reset, with window and 2x2); `carry/` (2x/4x/16x and 1/256x at
±32767/−32768, the exact remainder cap and the dropped excess, curve-off pass-through of
−32768 for disabled/bypass/identity/other DPI stage); `idle/` (default and idle bytes 1, 2,
10, 255 at idle − 1 / idle / idle + 1, zero reports, each of those three gaps across the
u32 wrap, backwards clock); `dpi/` (sensor DPI unknown/base/other/unchanged with and without ONLY_AT_BASE_DPI);
`set-profile/` (identity vs non-identity remainder survival, out-of-range fields);
`misc/` (notify_idle, reset_remainders, set_clock_ms, set_window_counts, `Engine::new`
base CPI 0/1/49/50/51/65535); `headroom/` (normalize: boost, no boost, exactly 1.00x
with gain 512, 65535 clamp, rounding, multiplier floor); `ccw/` (frame round trips);
`hid/` (every opcode and error path: framing, version, CRC, SET_PROFILE rejections, SAVE_FLASH without/true/false,
SET_ENABLED 0/1/2/3/255, SIMULATE_DELTA incl. −32768, ENTER_BOOTLOADER wrong
magic/no callback/ok, unknown opcodes); `random/` (fixed-seed fuzzer mix, three HID-heavy).
