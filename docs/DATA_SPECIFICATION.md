# DATA_SPECIFICATION — KineForge session data

Status: DRAFT, schema `kineforge.session/0.1`, raw log format `KFLOG v1`
(2026-11-14, revised after two independent review rounds). Research-phase
proposal: names and layouts may change before 1.0. After 1.0, changes follow
§11.

## 1. Principles

1. **Raw observations are immutable.** The raw log is append-only and is never rewritten, truncated or "repaired" in place. Derived data lives in separate files that reference the raw log by SHA-256.
2. **Every number has a unit, a frame, a clock and an evidence class.** Units are in field names (`_ns`, `_rad`, `_m`, `_px`, `_hz`); frames, clocks and evidence classes are declared per source/stream.
3. **Provenance is mandatory.** An input source without an `input_level` is invalid; data from different input levels is never merged into one source.
4. **Local-first.** A session is a self-contained bundle that can be copied, exported and imported without any server.
5. **Readable without KineForge.** JSON, Parquet, ZIP, and a documented byte-level log format with a published stdlib-only reference reader. Every on-disk enum has explicit numeric values (§6.7).
6. **The raw log alone is sufficient.** It embeds the session header, declarations, commit watermarks and end record; every other file can be regenerated from it.
7. **Replay reproduces what the live run consumed**, frame by frame (§9), not an idealised re-sort.

Two label sets exist and must not be confused:
- `evidence_class` (§3.2) labels **data** (observed, app-reported, inferred, simulated truth, derived).
- `claim_status` (EXPERIMENT_PLAN.md §0.2) labels **claims and constants** (VERIFIED, EXPERIMENTALLY OBSERVED, INFERRED, COMMUNITY-ESTABLISHED, UNKNOWN, REQUIRES FURTHER VALIDATION) — e.g. a sensitivity-profile constant.

## 2. Conventions

### 2.1 Time

| Concept | Representation |
|---|---|
| Session time `t_ns` | `i64` ns on the **session clock**: the app's monotonic clock (`std::time::Instant` natively, `performance.now()` on the web), zero at session start. For input records it is the **receipt** time (always present). |
| Source timestamp `src_ts_ns` | `i64` ns in the source's own clock domain, unit-converted but not offset; 32-bit millisecond sources (X server, `wl_pointer`) are unwrapped to 64 bits. **`i64::MIN` = the source has no timestamp of its own** (Windows Raw Input, hidraw). |
| Clock domain | `{id, kind, meaning, resolution_ns, offset_to_session_ns, offset_uncertainty_ns, drift_ppm}`. `kind` ∈ `session`, `linux_monotonic`, `linux_realtime`, `linux_boottime`, `x_server_ms`, `wayland_utime`, `gameinput_us`, `mach_absolute`, `browser_perf` (+ `coi: bool`). `meaning` ∈ `session_clock` (domain 0 only; offsets 0), `kernel_flush` (evdev/libinput), `kernel_event` (IOHID AbsoluteTime), `os_dispatch` (X server; Chrome-on-X11 event times), `compositor` (Wayland), `browser_receipt` (Chrome on Windows/Wayland). Values from EXPERIMENT_PLAN E0 and `clock_sync` records. |
| Receipt meaning | Per source, `receipt_meaning` ∈ `app_dequeue` (capture-thread stamp at drain/read), `dom_callback` (web handler entry), `imported` (timestamps from an imported file) — describes `t_ns`. |
| Wall clock | RFC 3339 UTC in the header (`started_at_utc`); omitted in privacy exports. |
| **Simulation time τ** | Advances only while the simulation runs. `tick: u64` counts steps of p = 1e9/`sim_hz` ns (`sim_hz` must divide 1e9 and be at most 10 000; default 1000); tick k ends at the instant (k+1)·p, where its work happens (time limit, timeouts, spawns), so τ after n ticks is n·p. **The `frame` records' `first_tick`/`ticks` are authoritative** — replay never recomputes ticks from timestamps. The live clock (`kf-sim`) is defined by frames: it starts running at session time 0 with remainder 0 (and the camera at unit totals 0); a running frame adds max(`t_input_cutoff_ns` − anchor, 0) (anchor = the previous running frame's cutoff, or the `t_ns` of the `resume` that ended a pause) to a sub-tick remainder and steps ⌊remainder/p⌋ ticks, at most `max_catch_up_ticks` (clamped to `sim_hz`; when capped, the frame is flagged and the remainder kept is p − 1, the rest of the stall dropped); paused frames step none. A frame never steps more than `sim_hz` ticks (one second). τ therefore excludes pauses (from the frame where `pause` is applied to the `resume` time) and time dropped by capped catch-up. |

Sentinels (`i64::MIN`, `u32::MAX`, `u64::MAX`, `0xFFFF`) are never used in arithmetic; Parquet export and the reference reader map them to null. In memory (`kf-core`) they are `Option`s, and writers refuse `Some(sentinel)`.

### 2.2 Coordinate frames

| Frame | Definition |
|---|---|
| **Device** | mouse sensor axes as reported in HID: +x = rightward hand motion, +y = motion **toward the user** (screen-down); integer counts |
| **Screen (physical)** | pixels, origin top-left, +x right, +y down |
| **Screen (CSS / DIP)** | CSS px (`css_px × DPR × zoom` = physical, Firefox) or DIP (`dip × DPR` = physical, Chrome) at event time |
| **World** | right-handed, metres; +X right, +Y up, −Z forward at yaw = 0 |
| **Camera** | yaw ψ about +Y, **positive = turn right** (clockwise seen from above); pitch θ, **positive = look up**, clamped to [−π/2, π/2]; roll 0. Forward = (sin ψ cos θ, sin θ, −cos ψ cos θ). Radians (`f64`) in data, degrees only in UI |
| **Target-relative** | (Δψ, Δθ) of the target centre from the crosshair in the camera's yaw/pitch frame, plus great-circle angle `ang_err_rad` |

### 2.3 Mouse → camera and simulation inputs (normative; applies when `simulation` is present)

1. **Driving source.** The header names the *initial* driving source (`simulation.input_source`), which is declared before the first `frame` record (same group or earlier); each `drain` record names the source it consumed and is authoritative. The driving source may change only between a `pause` and a `resume` (recorded as `sim_control` `switch_source`) and only to a source with **identical `input_level`, `units` and `value_scale`**. If a re-lock or device change would produce anything else, the session ends (`end_reason: input_level_changed`) and a new session starts. Other sources running at the same time are record-only. The recorder cuts a group just before a frame message that carries `switch_source` and forwards the new source's events to the simulation from then on, so the new source consumes exactly its events recorded from that frame's group on (§9.1); the old source's undrained events are discarded. A `switch_source` naming the source already driving changes no input (it only resets the gain state, item 3).
2. Each applied motion record contributes `(dx / value_scale, dy / value_scale)` in the source's units (exact: value_scale is 1, 256 or 1024). Records flagged **absolute are never applied** (and are routed to a separate `os_absolute_pointer` source anyway).
3. **Directional gain** (`kf-dirgain`, ODD-compatible) only when the driving source has `units = counts`, `value_scale = 1`; applied per event in `seq` order; integer output. Its millisecond clock input is `⌊src_ts_ns / 10⁶⌋` (wrapping to u32) when the record has a source stamp, else the same of `t_ns` (processing flag `approx_gain_timing`). A record whose `dx`, `dy` fit `i16` (every HID report, −32768 included) is one engine call; a wider one is fed as successive pieces of at most ±32767 per axis (no count is lost). The engine's sensor DPI is 0 (unknown), so a profile's `ONLY_AT_BASE_DPI` flag has no effect; `switch_source` resets the engine's remainders and direction estimate.
4. **Accumulation.** `yaw_units`, `pitch_units` are `i64`. ψ = yaw_units × (yaw_rad_per_unit × sens_x) / value_scale; θ = −pitch_units × (pitch_rad_per_unit × sens_y) / value_scale (device +y is toward the user = look down; `invert_y` flips the sign). The pitch clamp is applied **in unit space per event** (path-dependent ⇒ events are applied one by one, never pre-summed).
5. **Shots.** A down record of button 0 (left) that is applied (not discarded while paused) while a trial is running fires a shot; other buttons, and button-downs outside a running trial, fire none. Its ray uses `yaw_units/pitch_units` **immediately after the last applied motion record with a lower `seq`** — independent of how events were batched into frames. Every applied event has the simulation time τ = `first_tick`·p + r + (t − anchor), with r the clock's sub-tick remainder at the frame's start, t = its `t_ns` clamped to [anchor, `t_input_cutoff_ns`] and τ kept below (`first_tick` + `ticks` + 1)·p; without pauses or capped catch-up τ = t exactly. Target positions are evaluated at τ clamped to the target's [spawn, despawn] instants (analytic: `pos_m + vel_m_s × Δt`). The hit target is the one with the smallest ray entry distance; the shot's **reference target** is the hit one, else the alive target with the smallest angle to the ray (ties: lowest id), else none.
6. **Immutability.** Sensitivity, FOV, gain profile and `invert_y` are immutable within a session; multi-condition studies record one session per block, linked by the `experiment` object (§7).
7. **Everything else goes through `sim_control`.** Every simulation state change that does not come from driving-source records and is requested from outside the simulation — trial start/restart (keyboard or UI), pause/resume, abort, block advance, source switch, end — is a `sim_control` record (§6.4). Changes the scenario itself makes as simulated time passes (spawns, timeouts, a trial completing) are results of the simulation, not controls.

### 2.4 Mouse units

`cpi` (counts per inch; vendors say "DPI") is **declared** unless measured: `cpi_declared`, and if measured, `cpi_measured` + method + uncertainty. 1 in = 25.4 mm exactly. For a counts source (value_scale 1): cm/360 = 2.54 × 2π / (yaw_rad_per_unit × sens × cpi).

## 3. Measurement taxonomy

### 3.1 `input_level` (per source; never mixed)

Levels follow the pipeline taxonomy L1–L7 (TECH_STACK_RESEARCH.md §3.4); a sample is labelled with its **lowest applicable** layer.

| Value | Layer | Meaning | Sources |
|---|---|---|---|
| `device_report` | L1 | integer X/Y of a HID input report, parsed with the report descriptor, one value per report (firmware processing already inside) | Linux hidraw, macOS IOHID report callback (Input Monitoring, non-seize), USB bus capture, instrumented firmware log |
| `os_raw_counts` | L2 | OS relative events carrying the HID logical values, one packet per report, no ballistics | Linux evdev `EV_REL` (zero axes dropped; all-zero reports invisible); Windows Raw Input **relative** packets with no filter driver detected |
| `os_unaccelerated` | L2 | pre-ballistics OS deltas that may be rotated, scaled, aggregated or plugin-modified | libinput `dx_unaccelerated`, X11 `XI_RawMotion` raw values, Xwayland raw (surface-scaled), Wayland `dx_unaccel`, macOS unaccelerated CGEvent field 170 / GCMouse, Windows GameInput |
| `os_absolute_pointer` | L2† | absolute coordinates through a raw API (RDP, VMs, tablets) — never deltas | Windows Raw Input `MOUSE_MOVE_ABSOLUTE` |
| `os_pointer` | L3 | post-ballistics pointer motion in display space | `WM_MOUSEMOVE`/`WM_POINTER`, X core/`XI_Motion`, `wl_pointer`, `NSEvent.deltaX` |
| `browser_locked_unadjusted` | L4u | Pointer Lock where the `unadjustedMovement: true` request **resolved** | Chrome/Firefox on Windows & macOS, Safari macOS, Chrome ChromeOS |
| `browser_locked` | L4a | Pointer Lock without unadjusted movement (OS acceleration applies) | all browsers; the only locked mode on Linux |
| `browser_unlocked` | L4p | cursor positions — UI only, never aiming analysis | all |
| `synthetic` | — | generated inside KineForge (test patterns, bots), seed recorded | generator |
| `replayed` | — | re-emitted from a recording; references the original log hash | replay |

An `os_unaccelerated` source may carry `counts_equivalent_verified: <experiment id>` once a dual-source ratio test against an L1 source (EXPERIMENT_PLAN E1) shows a ratio of exactly 1 on that system.

A source declaration (`source_declare`, §6.1) contains: `id` (0–0xFFFE; 0xFFFF means "none" in annotations and `sim_control`), `source_rank` (cross-source ordering for analysis; default = `id`), `device`, `input_level`, `units` (`counts`, `os_units`, `logical_px`, `dip`, `css_px`, `points`, `abs_65535`) with `value_scale`, `clock_domain`, `receipt_meaning`, `evidence_class`, `processing` flags (`os_accel_off_verified`, `os_accel_state_unknown`, `rotation_applied`, `dpr_scaled`, `zoom_scaled`, `coalesced`, `fractional`, `background_throttled`, `filter_driver_suspected`, `injected_test_stream`, `hid_bpf_present`, `libinput_plugins_possible`, `approx_gain_timing`), the backend (`Source` variant), granted capture-thread priority, and for browsers the lock request result (`unadjusted_movement: resolved | rejected:<error>`), the motion event type (`pointerrawupdate` | `pointermove`), DPR and zoom. Static OS configuration (acceleration profile/speed, libinput version, mouse-class `UpperFilters`, `SM_REMOTESESSION`) is included where known.

### 3.2 `evidence_class`

| Value | Meaning |
|---|---|
| `observed` | directly captured from a sensor/OS/browser API (or imported from a capture tool's file) |
| `app_reported` | displayed/reported by an application (e.g. a third-party results screen), transcribed or captured |
| `inferred` | produced by a model from observations; references the model/experiment |
| `simulated_truth` | ground truth from KineForge's own simulation |
| `derived` | statistics computed from other data; references inputs by hash + algorithm version |

Sources with `input_level: synthetic` declare `simulated_truth`; `replayed` sources keep the evidence class of the source they re-emit.

For third-party sessions (e.g. Aimlabs), target positions and camera state are **absent** unless obtained by a permitted, verified method; they are never filled with estimates labelled as observations.

## 4. Bundles

```
sessions/<session-id>/
  raw.kflog                  # source of truth (never modified after close)
  raw.recovered.json         # only after crash recovery (§5.4)
  manifest.json              # convenience copy of header + declarations (up to 256 KiB of declaration JSON; declarations_omitted counts the rest) + end record + file hashes (regenerable)
  derived/
    input_motion.parquet  input_events.parquet  frames.parquet  frame_timing.parquet  drains.parquet
    sim_camera.parquet    sim_targets.parquet   sim_shots.parquet  sim_control.parquet  annotations.parquet
    app_results.parquet   clock_sync.parquet    (trials.parquet  metrics.parquet: computed tables, M9)
analyses/<analysis-id>/      # cross-session artifacts (§8.1)
  manifest.json  *.parquet  [profile.odd.bin  profile.odd.json]  report.md
export: <id>.kfz             # ZIP, members STORED, flat names (§12.1): raw.kflog, manifest.json, <table>.parquet, FORMAT.md (text)
```

`session-id` and `analysis-id` are **UUIDv4** (a UUIDv7 would leak the creation time even in privacy exports). The **session identity hash** is SHA-256 of `raw.kflog`, or for recovered sessions SHA-256 of the recovered byte stream (§5.4); derived tables, analyses and the validator use it.

## 5. Raw log format `KFLOG v1`

All integers little-endian; floats IEEE-754 binary64 LE; NaN and ±infinity invalid everywhere. The byte-level reference, normative where this section is less precise, is `crates/kf-telemetry/FORMAT.md` (shipped in every `.kfz`).

### 5.1 Layout

**File header (16 B):** magic `4B 46 4C 4F 47 0D 0A 1A` (`"KFLOG\r\n\x1a"`), `u32 format_version = 1`, `u32 reserved = 0`.

**Chunk** = 16-byte header + payload + 4-byte CRC + 4 zero bytes (stride = 24 + `payload_len`; every chunk starts 8-byte aligned):

| Field | Type | Notes |
|---|---|---|
| `payload_len` | u32 | multiple of 8; ≤ 1 MiB |
| `kind` | u8 | record kind (§6) |
| `kind_version` | u8 | layout version of that kind (1 for every v1 kind; a known kind at another version is skipped like an unknown kind) |
| `flags` | u16 | bit 0 = payload LZ4-compressed (reserved; v1 writers never set it) |
| `record_count` | u32 | fixed-layout kinds: `payload_len == record_count × record_size`; JSON kinds and `commit`: always 1; `annotation`: the number of records |
| `reserved` | u32 | 0 |
| `payload` | bytes | packed fixed-size records of one kind (all sizes are multiples of 8), or **exactly one** UTF-8 JSON document padded with trailing spaces to a multiple of 8, or (annotation) self-framed variable records |
| `crc32` | u32 | CRC-32 (IEEE, as Python `zlib.crc32`) over the 16-byte chunk header and the payload |

### 5.2 Writing

- **Producers.** Capture events arrive at the recorder (ARCHITECTURE.md §3); the main loop sends **one message per frame** containing all records produced in that frame (frame, drain, sim_*, sim_control, annotations, frame_timing of earlier frames). The recorder appends to per-kind buffers and **cuts a group only at a frame-message boundary**, every `chunk_flush_ms` (default 100 ms), when any buffer reaches 4096 records, and just before a frame message that carries `switch_source` (§2.3 item 1).
- **Group order:** declaration chunks (`session_header`, `source_declare`, `device_info`) first, then the other kinds in ascending kind number, then the `commit` chunk last. Within a kind, records keep arrival order.
- **Commit watermark:** `t_ns`, `tick` = last tick of the last complete frame written so far, `frame` = that frame (both `u64::MAX` before any frame), and per-source last `seq` written so far.
- **Native durability:** a writer thread `write`s each group; a separate sync thread calls `fdatasync` about every `fsync_ms` (default 1000 ms; each sync starts early by the previous one's duration, so with steady syncs shorter than half the interval a write is durable about `fsync_ms` after it; in general within max(`fsync_ms`, longest sync) + longest sync, the longest sync being reported by the writer) and at close, so writes never wait for a sync. The recorder→writer queue is unbounded up to a memory cap (default 64 MiB); beyond it, a group is dropped and the session ends with `storage_error` (as for a write error, below). The maximum observed backlog is reported in `session_end`.
- **Recorder memory:** whenever the recorder thread reads the writer's status it passes `written_bytes` to `Recorder::written`, which forgets group boundaries that can no longer be rolled back to, so memory stays bounded however long the session runs.
- **Web durability:** chunk bytes go to the OPFS Worker, which `write`s them and calls `flush()` every `fsync_ms`. On `visibilitychange: hidden` and `pagehide`, the main thread sends its frame message, the recorder cuts a complete group **with its commit**, and the worker is asked to flush.
- **Liveness lock:** natively an exclusive file lock (`flock`/`LockFileEx`); on the web the writing tab holds `navigator.locks.request(session_id)` for the whole session. A held lock means "live — do not recover".
- **Space reserve:** at session start the writer creates a reserve file of ≥ 64 KiB of random bytes next to the log (`raw.kflog.reserve`, synced; web: the same in OPFS) — portable, instead of `fallocate(FALLOC_FL_KEEP_SIZE)`/`FileAllocationInfo`; zeros would take no space on compressing filesystems. On a write error (`ENOSPC`, `QuotaExceededError`, any I/O error) the writer cuts the torn group off (it was never committed) and drops later groups. **After any drop** the recorder waits for the writer to drain, rolls back to the file's last group boundary (`Recorder::writer_lost`: counts and commit watermarks as written), and records a `storage_error` gap per source with written input plus one without a source (covering sources whose records were all lost). The writer releases the reserve right before it writes the final group — those gaps, its commit and `session_end` (`end_reason: storage_error`, `dropped_chunks` counted) — into the freed space.
- **Close sequence:** the recorder cuts and submits its last regular group like any other and waits for the writer to drain (a storage error seen there ends the session with `storage_error`); the final group then holds only its `commit` (plus any `storage_error` gaps) and `session_end` as the terminal chunk, so it always fits the reserve. Recovery treats a CRC-valid `session_end` directly after a `commit` as committed.
- **Expected loss bounds** (PERFORMANCE_PLAN N9b/N9c): process kill or tab close ≤ `chunk_flush_ms` + writer backlog at kill time; power loss ≤ max(`fsync_ms`, longest sync) + longest sync + `chunk_flush_ms` + backlog (≈ `fsync_ms` + `chunk_flush_ms` + backlog on a disk whose syncs are short and steady).

### 5.3 Forward compatibility

Readers skip unknown kinds (by `payload_len`) and report them. A layout change bumps `kind_version`; old versions stay readable forever.

### 5.4 Recovery (never in place)

Only when the liveness lock is free; recovery holds it (shared, so it works on read-only and NFS file descriptors) while it runs and deletes a leftover `raw.kflog.reserve`. Scan from the start; stop at the first short read, bound violation, CRC mismatch, or non-zero trailer/reserved bytes; the valid prefix ends at the **last `commit` chunk** before that point. Build a deterministic synthetic `session_end` (`{completed: false, end_reason: "recovered", t_end_ns: <last commit t_ns>, counts: <committed records>, …}`, every field determined by the prefix; FORMAT.md §8) and write `raw.recovered.json` = `{valid_prefix_bytes, synthetic_end_b64, recovered_sha256, tail_bytes_ignored}` next to the untouched `raw.kflog`. The recovered byte stream is *prefix ‖ synthetic end chunk*; its SHA-256 is the session identity; exports write exactly those bytes. A sidecar is ignored if `raw.kflog` contains a CRC-valid `session_end` directly after a commit (the session was not dead after all). Records after the last commit are reported as uncommitted and never used.

## 6. Record kinds (v1)

### 6.1 Framing, declarations, annotations

| kind | Name | Layout | Size |
|---|---|---|---|
| 0x00 | `session_header` | JSON (§7); first chunk; only facts known at start | var |
| 0x04 | `source_declare` | JSON source declaration (§3.1); precedes (same or earlier group, in file order) the first record using the id. A **new source id is opened on every pointer-lock acquisition** | var |
| 0x05 | `device_info` | JSON (VID/PID/interface, CPI, report rate, firmware version when readable) | var |
| 0x06 | `render_state` | JSON `{surface_px, dpr, crosshair_offset_px: [x, y], present_mode, frame_cap_hz}` — at start and on every resize/DPR/present-mode change | var |
| 0x7C | `commit` | `t_ns i64, tick u64, frame u64, n u32, _pad u32, n × {source u16, _pad u16, last_seq u32}` | 32 + 8n |
| 0x7D | `clock_sync` | `t_ns i64, domain u16, _pad u16, _pad u32, src_ts_ns i64, uncertainty_ns i64` | 32 |
| 0x7E | `annotation` | sequence of `{t_ns i64, code u16, source u16 (0xFFFF = none), first_seq u32, lost u32 (u32::MAX = unknown), len u32, utf8[len], pad to 8}` | var |
| 0x7F | `session_end` | JSON `{completed, end_reason, t_end_ns, counts per kind, dropped_chunks, max_writer_backlog_bytes, interruptions, refresh_hz_measured, raf_hz_measured}` (the last two omitted when unknown); terminal chunk | var |

### 6.2 Input (evidence class per source declaration, §3.2)

| kind | Name | Layout | Size |
|---|---|---|---|
| 0x01 | `input_motion` | `t_ns i64, src_ts_ns i64, dx i32, dy i32, seq u32, source u16, flags u16` | 32 |
| 0x02 | `input_button` | `t_ns i64, src_ts_ns i64, seq u32, source u16, button u8, state u8` | 24 |
| 0x03 | `input_wheel` | `t_ns i64, src_ts_ns i64, dv i32, dh i32, seq u32, source u16, flags u16` | 32 |

- **`seq`**: one u32 counter per source shared by motion, button and wheel, assigned in arrival order; within one HID report / RAWMOUSE packet motion precedes the button record. It wraps after 2³² events (≈6 days at 8 kHz); readers unwrap it.
- Motion/wheel `flags`: bit 0 coalesced sub-event, bit 1 background (`RIM_INPUTSINK`), bit 2 absolute packet, bit 3 suspected injected (`hDevice == 0`, `LLMHF_INJECTED`, virtual device), bit 4 clamped by the capture layer, bit 5 hi-res wheel.
- Button `state`: bit 0 down, bit 1 background, bit 2 suspected injected, bit 3 clamped. `button`: 0 left, 1 right, 2 middle, 3 back (X1), 4 forward (X2), 5+ device-specific.
- `dx/dy` are integers × `value_scale` (counts 1; Wayland `wl_fixed` 256 exact; fractional browser/macOS values 1024, rounded, `fractional` flag).
- Browser capture listens to exactly **one** motion event type per session and records only **leaf** coalesced events (WEB_PLATFORM_RESEARCH.md §2.8).
- Raw input is never smoothed, resampled or interpolated in the log. **OS-side loss is invisible to `seq`** on Windows Raw Input and in browsers; it is detectable only by Σcounts against an L1 source.

### 6.3 Frames and drains

| kind | Name | Layout | Size |
|---|---|---|---|
| 0x10 | `frame` | `frame u64, t_start_ns i64, t_input_cutoff_ns i64, first_tick u64, ticks u32, flags u16, _pad u16` — written when the frame's simulation step completes; **authoritative for replay** | 40 |
| 0x11 | `drain` | `frame u64, source u16, _pad u16, last_seq u32` — written only for frames that consumed ≥ 1 driving-source event | 16 |
| 0x12 | `frame_timing` | `frame u64, t_submit_ns i64, t_present_ns i64 (i64::MIN = unknown), cpu_ns u32, gpu_ns u32 (u32::MAX = unknown)` — written when timings resolve (possibly frames later) | 32 |

`t_present_ns` is filled only where the platform reports real presentation time (most do not); it is never estimated in the raw log.

### 6.4 Simulation (evidence `simulated_truth`)

| kind | Name | Layout | Size |
|---|---|---|---|
| 0x20 | `sim_camera` | `tick u64, yaw_units i64, pitch_units i64, yaw_rad f64, pitch_rad f64` (on change, ≤ once per tick) | 40 |
| 0x21 | `sim_target` | `tick u64, target u32, event u8, shape u8, _pad u16, pos_m [f64;3], vel_m_s [f64;3], radius_m f64` | 72 |
| 0x22 | `sim_shot` | `tick u64, t_ns i64, target u32 (u32::MAX = none), seq u32 (button record), ang_err_rad f64, d_yaw_rad f64, d_pitch_rad f64, hit u8, _pad [u8;7]` | 56 |
| 0x23 | `sim_control` | `tick u64, t_ns i64, frame u64, action u8, phase u8 (0 before drain, 1 after drain), reason u16, new_source u16 (0xFFFF = n/a), _pad u16` | 32 |

`yaw_units/pitch_units` make camera state exactly reconstructible; `yaw_rad`/`pitch_rad` must equal the recomputation bit-for-bit. Events drained while paused are discarded by the simulation but still advance the drain's `last_seq`.

**Ticks in simulation records** (KineForge `kf-sim`): `sim_camera` at tick k is the camera in effect when tick k is stepped, written when it differs from the previous record (changes after the last stepped tick are never in effect); `sim_target` spawn/despawn (timeout, trial end) records carry the tick whose work did it, and despawns caused by a control (trial end by `abort_trial`, `end`, a new start) the control's tick; changes caused by an event (shots, kills) carry the tick about to be stepped when the event applied — ⌊τ/p⌋ when that tick has not passed yet, else the frame's current tick (events never step ticks back); `sim_control.tick` is the next tick when the control was applied (`first_tick` for phase 0, `first_tick + ticks` for phase 1). `sim_shot.target` is the reference target (§2.3 item 5; `u32::MAX` only when no target was alive, then `ang_err_rad`, `d_yaw_rad`, `d_pitch_rad` are 0.0 meaning n/a); `kill` and `despawn` records carry the target's position at that time. A kill or timeout frees a slot; in `flick.static` the replacement spawns in the work of the tick the resolution is recorded at (a timeout's own tick; for a kill, the first tick instant after it).

### 6.5 External / app-reported (evidence `app_reported`)

| kind | Name | Layout |
|---|---|---|
| 0x30 | `app_result` | JSON `{t_ns?, metric, value, unit, method (manual_transcription / screenshot / owner_export), transcribed_by}` |

### 6.6 Data rates (arithmetic, not measurements)

Per active source at 8 kHz: 8000 × 32 B = 256 KB/s. Research configuration (three simultaneous 8 kHz sources): ≈ 768 KB/s; plus frames + timings at 360 Hz × 72 B, `sim_camera` ≤ 1000 × 40 B/s, drains, targets → **≈ 0.85 MB/s ≈ 3 GB/h worst case**. Typical sessions last minutes. Browser memory, not bandwidth, is the constraint: the log streams to OPFS.

### 6.7 Numeric enum tables (frozen once 1.0 ships; never renumbered)

| Enum | Values |
|---|---|
| annotation `code` — gaps (0x0001–0x00FF) | 0x0001 kernel_dropped, 0x0002 queue_overflow, 0x0003 sim_queue_overflow, 0x0004 record_queue_overflow, 0x0005 pointer_lock_lost, 0x0006 focus_lost, 0x0007 visibility_hidden, 0x0008 background_throttled, 0x0009 device_removed, 0x000A clock_changed, 0x000B storage_error |
| annotation `code` — state (0x0100–0x01FF) | 0x0100 focus_gained, 0x0101 visibility_visible, 0x0102 pointer_lock_acquired, 0x0103 device_added, 0x0104 context_lost, 0x0105 context_restored, 0x0106 frame_drop |
| annotation `code` — experiment (0x0200–0x02FF) | 0x0200 block_start, 0x0201 block_end, 0x0202 condition, 0x0203 warmup_start, 0x0204 warmup_end |
| annotation `code` — other | 0x0F00 user_marker |
| `sim_control.action` | 0 start_trial, 1 restart_trial, 2 pause, 3 resume, 4 abort_trial, 5 end, 6 next_block, 7 switch_source |
| `sim_control.reason` | 0 user, 1 pointer_lock_lost, 2 focus_lost, 3 visibility_hidden, 4 context_lost, 5 sim_queue_overflow, 6 storage_error, 7 input_level_changed, 8 scenario_complete, 9 experiment_schedule |
| `sim_target.event` | 0 spawn, 1 update, 2 despawn, 3 hit, 4 kill |
| `sim_target.shape` | 0 sphere, 1 capsule, 2 box |
| `frame.flags` | bit 0 paused (after the frame's phase-0 controls: it stepped no ticks), bit 1 tick catch-up capped, bit 2 render skipped |
| `button` | 0 left, 1 right, 2 middle, 3 back, 4 forward |

Unknown values are preserved and reported, never dropped.

## 7. Session header (kind 0x00) — example (Chromium 156, Linux X11, accelerated lock)

```json
{
  "schema": "kineforge.session/0.1",
  "session_id": "6f1c2b9e-4d7a-4f3e-9c21-0a5b7d3e8f11",
  "started_at_utc": "2026-11-14T21:04:05Z",
  "app": {"name": "kineforge-trainer", "version": "0.1.0", "git": "abc1234", "target": "wasm32-unknown-unknown", "rustc": "1.100.0"},
  "source_app": null,
  "platform": {
    "os": "linux", "session_type": "x11",
    "browser": {"name": "chromium", "version": "156.0.8078.79"},
    "gpu": {"api": "webgl2", "fallback_used": true, "adapter": null},
    "display": {"refresh_hz_declared": 165.0, "physical_px": [2560, 1440]}
  },
  "capabilities": {"pointer_lock": true, "cross_origin_isolated": true, "webhid": false, "opfs_sync": true},
  "render": {"present_mode": "fifo", "frame_cap_hz": null, "desired_maximum_frame_latency": null},
  "clock_domains": [
    {"id": 0, "kind": "session", "meaning": "session_clock", "resolution_ns": 5000, "offset_to_session_ns": 0, "offset_uncertainty_ns": 0, "drift_ppm": 0},
    {"id": 1, "kind": "browser_perf", "coi": true, "meaning": "os_dispatch", "resolution_ns": 1000000,
     "offset_to_session_ns": -1834512000, "offset_uncertainty_ns": 5000, "drift_ppm": 0}
  ],
  "simulation": {"sim_hz": 1000, "input_source": 0,
                 "rng": {"algorithm": "xoshiro256++", "seeding": "splitmix64", "seed": "0x5eed0001"}},
  "sensitivity": {
    "model": "linear", "yaw_rad_per_unit": 0.0003839724354387525, "pitch_rad_per_unit": 0.0003839724354387525,
    "sens_x": 1.0, "sens_y": 1.0, "invert_y": false,
    "directional_gain": null,
    "fov": {"value_deg": 103.0, "model": "hor_plus", "basis_aspect": [16, 9]}
  },
  "scenario": {"id": "flick.static", "version": 1, "config": {"…": "inline"}, "config_sha256": "…"},
  "scoring": {"model": "kf.flick.v1", "scoring_version": 1},
  "experiment": {
    "id": "sens-ab-2026-11", "protocol_version": 1,
    "prereg": {"hypothesis": "…", "primary_metric": "endpoint_error_rad", "analysis_plan_sha256": "…", "registered_at": "2026-11-10"},
    "participant": "p-7f3a",
    "session_index": 2, "condition_id": "B", "block_index": 3, "block_order_scheme": "ABBA", "warmup": false, "deviations": []
  }
}
```

The source declaration that follows (a new one, with a new id, on every re-lock):

```json
{"id": 0, "source_rank": 0, "device": null, "kind": "mouse", "backend": "WebCapture",
 "input_level": "browser_locked", "units": "dip", "value_scale": 1,
 "processing": ["os_accel_state_unknown", "coalesced"], "clock_domain": 1, "receipt_meaning": "dom_callback",
 "unadjusted_movement": "rejected:NotSupportedError", "motion_event": "pointerrawupdate",
 "device_pixel_ratio": 1.0, "page_zoom": 1.0, "evidence_class": "observed"}
```

Notes: Chrome's adjusted `movementX` on Linux is integer DIP (not CSS px, not fractional); Chrome-on-X11 event timestamps are X-server milliseconds (`os_dispatch`, effective resolution 1 ms) before page coarsening; `offset_to_session_ns` is minus the `performance.now()` value at session start. The sensitivity (0.022°/count in radians) is illustrative. `fov.model` says how the value scales with the render aspect (`kf-core` `FovModel`): `hor_plus` — the value is the horizontal FOV at `basis_aspect` and the vertical FOV is kept at other aspects (Source engine: basis 4:3); `vert_minus` — the value is the horizontal FOV at every aspect; `vertical` — the value is the vertical FOV at every aspect. `participant` is a random pseudonym, never personal data. Directional gain, when enabled, is `sensitivity.directional_gain = {"profile": <the ODD 28-byte v1 profile as 56 lowercase hex digits>, "engine": "kf-dirgain/odd-c177d23", "policy": "carry_capped" | "drop"}` — `carry_capped` is ODD's (excess carried up to ≈32767 counts, dropped beyond — OPEN_DIRECTIONAL_DPI_AUDIT.md §6); `kf-sim` implements only that one. `simulation.rng.seed` is the 64-bit seed as a `"0x…"` hex string. `scenario` = `{id, version, config, config_sha256?}` with **every** config field present (no defaults are filled in on replay) and no others: a scenario config is closed and versioned by `scenario.version` (an exception to §11's "readers ignore unknown fields": a new field means a new version); `flick.static` v1 is documented in `kf-sim` (`FlickConfig`: targets, concurrent (≤ 64), distance_m, offset_min/max_rad from the home direction, directions (balanced set, dividing targets), radius_min/max_rad (angular), lifetime_ms, time_limit_ms; trial i uses the seed's xoshiro stream jumped i times). `scoring` = `{"model": "kf.flick.v1", "scoring_version": 1}`: per attempt (a `start_trial`/`restart_trial` and what followed), counts of shots, hits, kills, timeouts and shots without a target, accuracy, kills/s, mean time to kill and mean angular error (each absent when its denominator is 0), and for complete attempts score = ⌊100 × kills × hits × 10⁹ / (shots × duration_ns)⌋ (100 × kills/s × accuracy, integers; duration from the first spawn instant).

**Float exactness.** Simulation-critical JSON floats must round-trip exactly: the workspace enables serde_json's `float_roundtrip` feature (tested by round-tripping random `f64`s); Python's `json` parses correctly rounded.

### 7.1 External sessions (third-party applications)

`source_app` = `{name, build, platform, compat_layer (e.g. "Proton 11"), capture_method, settings_as_entered: {...}}`, values `app_reported`. There is no `simulation` object: the driving-source, replay and recomputation rules (§2.3, §9, §10) apply only when `simulation` is present. Input records come from KineForge's own capture or from the imports below (labelled with their tool); `sim_*` records are absent.

| Import | Mapping |
|---|---|
| MouseTester CSV (`xCount,yCount,Time (ms),buttonflags`; Windows Raw Input) | source `input_level: os_raw_counts`, `units: counts`, `src_ts_ns = i64::MIN`, `receipt_meaning: imported` (MouseTester's app-dequeue ms, printed with 1 µs digits), **y negated back** (MouseTester negates Y) |
| ODD `record-evdev.ts` CSV | `os_raw_counts`, `src_ts_ns` in `linux_realtime` (`kernel_flush`) |
| KovaaK's `Stats.csv` | `app_result` records |
| Aimlabs results screens | `app_result` records, `method: manual_transcription` |

## 8. Derived data

Each Parquet table carries key-value metadata: `kineforge.format_version`, `session_identity_sha256`, `algorithm`, `algorithm_version`, `params` (JSON), `evidence_class`. Tables that re-encode raw records keep the raw evidence class (`input_motion` and `input_events` can mix sources, so theirs is a JSON object mapping each source id with rows to its declared class, `null` if undeclared, e.g. `{"0":"observed"}`); computed tables (`trials`, `metrics`, `flicks`, `tracking_error`, `directional_bias`, `fitts`, `scores`) are `derived`. Recomputing from the same raw log and algorithm version gives identical values (bit-exact for scoring kernels; declared tolerance for floating-point analysis kernels).

### 8.1 Analysis artifacts (cross-session)

`analyses/<id>/manifest.json` = `{kind: calibration | study | equivalence | benchmark, inputs: [{session_id, session_identity_sha256}], algorithm, algorithm_version, params, evidence_class: derived | inferred, input_levels_used, created_by}` plus outputs (Parquet tables, validation statistics; for calibrations the ODD 28-byte profile and JSON). The validator requires every input hash to resolve. Firmware-profile export is refused when the input levels violate the calibration-validity rule (EXPERIMENT_PLAN E8).

## 9. Replay semantics

### 9.1 Faithful replay, frame by frame

Live, in each frame F: (1) apply `sim_control` records of F with `phase = 0` (a paused simulation steps no ticks); (2) apply the driving source's events received up to `t_input_cutoff_ns`, one by one in `seq` order (events while paused are discarded; seqs listed in `sim_queue_overflow` annotations never reached the simulation), **interleaved with the frame's `ticks` ticks from `first_tick`**: before an event with simulation time τ (§2.3 item 5) every tick of the frame whose end instant is ≤ τ is stepped; (3) step the frame's remaining ticks; (4) apply `sim_control` records of F with `phase = 1`. Records: `frame`, `drain` (if ≥ 1 event), `sim_control`, `sim_*`. Frame numbers are consecutive.

Replay performs exactly the same steps using `frame.first_tick/ticks`, the `drain` (source, `last_seq`), the per-frame `sim_control` records and the `sim_queue_overflow` skip list. With the same build, scenario config and seed, `sim_*` and `sim_control` records are bit-identical (EXPERIMENT_PLAN E2/E10). Controls of one phase apply in file order. The camera a shot uses depends only on the `seq` prefix; without pauses or capped catch-up, and with every event's `t_ns` inside its frame's input window (after the previous cutoff, at most its own), the whole simulation is independent of how events were batched into frames (E2 checks it at 30–1000 Hz).

**What replay relies on** (writers follow it; `kf-cli replay` reports a log that breaks it as not replayable or, where the break is invisible to it, as a mismatch): a group holds whole frame messages; every input record a drain consumes and every `sim_queue_overflow` annotation that skips events of the driving source (with a source and a known `lost`) is recorded in the drain's group or an earlier one (the recorder logs before it forwards, and writes the annotation when the push to the simulation fails); a frame carrying `switch_source` is the first frame of its group (§2.3 item 1); a source's `seq`s strictly increase from group to group (a repeated or late record is not replayable); a `record_queue_overflow` (lost frame messages), an unknown `sim_control` action, a phase other than 0 or 1, non-consecutive frame numbers or a frame claiming more than `sim_hz` ticks make the rest of the session not replayable. Replay time is proportional to the simulated time the log claims (at most one second per frame record). Replay keeps only the driving source's undrained events across groups, so its memory does not grow with record-only sources.

### 9.2 Ordering

Within a source: `seq` (kind never reorders). Across sources (analysis only): `(t_ns, source_rank, seq)` with `source_rank` from each declaration. Simulation records: by `tick`; frames by `frame`.

### 9.3 Counterfactual replay

Same input, different sensitivity/gain/FOV model → new `sim_*` records labelled `simulated_truth` with `counterfactual_of = <session-id>`: what the camera *would* have done, not what the human would have done (no closed-loop correction). Always labelled as such.

### 9.4 Limits

Replay with a different build is allowed but reported (hashes need not match if simulation code changed). Third-party sessions cannot be reconstructed. Replay never uses the replaying machine's clock.

## 10. Validation

Magic, version; chunk bounds (`payload_len` ≤ 1 MiB and ≤ remaining bytes; fixed-kind size identity; JSON kinds `record_count = 1`); CRCs; group order (declarations first, commit last); declarations precede use in file order; commit watermarks monotonic; terminal `session_end` or a valid recovery sidecar; header JSON schema; `seq` gaps per source reported as loss (flagged as a capture bug when no gap annotation covers them; an annotation with unknown `lost` covers only the gap starting at its `first_seq`); `t_ns` monotonic per source (in `seq` order: a source's motion, button and wheel records sit in separate chunks); NaN and ±infinity absent; unknown string enum values in typed header/declaration fields are errors (they need a newer reader). When `simulation` is present: tick continuity across `frame` records; each `drain` is of the driving source, at most one per frame; driving-source switches only via `switch_source` between `pause` and `resume` with identical level/units/scale; `yaw_rad` equals recomputation from units; drains and `sim_control` are checked in replay order (§9.1), not file order. Replay equality of every `sim_*` and `sim_control` record is checked by `kf-cli replay` (`kf-sim`), not by the validator. The header checks (including the `session_id` UUID shape and the `started_at_utc` RFC 3339 UTC shape) are listed in FORMAT.md §10. A report keeps the first 1000 issues and counts the rest. Derived tables and analyses reference the session identity hash.

## 11. Versioning and migration

Schema id `kineforge.session/<major>.<minor>`. Minor = additive (new optional JSON fields, new record kinds, new enum values); readers ignore unknown JSON fields and skip unknown kinds. Major = breaking; a migration tool writes a **new** bundle with `migrated_from` and never modifies the original. Enum values (§6.7) are never renumbered or reused.

## 12. Privacy and security

- Stored by default: nothing that identifies a person or machine — no account, serial number, hostname, user name, IP, or full GPU string. VID/PID and GPU names are opt-in. Participant identifiers are random pseudonyms.
- Export shows exactly what is included; a "share benchmark" export keeps only anonymised derived metrics and omits `started_at_utc`.
- The default build contains no upload path.

### 12.1 Untrusted input (import of `.kfz` / `.kflog`)

Every length is bounded by the remaining bytes before allocation; chunks and JSON documents > 1 MiB are rejected; fixed-kind size identity enforced; ZIP members must be STORED, unique, flat names from a fixed allow-list (no paths, no `..`); the declared total size is checked against the storage quota before writing to OPFS (natively, `kf-cli unpack --quota` also counts the regenerated `manifest.json`, whose copies of declarations are capped at 256 KiB). Bundles never contain executable code: the reference reader `read_kflog.py` is published at a fixed location with its SHA-256 listed in `FORMAT.md`; recipients use the published copy.

### 12.2 Future sync (not in v1)

If ever added: opt-in, a separate feature/build, an append-only set union of immutable bundles keyed by `session_id` + identity hash (no merging), derived files regenerated rather than synced, the privacy export profile reused, and an explicit change to the web CSP `connect-src` (WEB_PLATFORM_RESEARCH.md §6).
