# KFLOG v1 and the KineForge session bundle — byte-level format

This file describes the raw session log `raw.kflog` (KFLOG v1, session schema
`kineforge.session/0.1`) and the session bundle well enough to read them
without KineForge. The implementation in the Rust crate `kf-telemetry` is
authoritative; `read_kflog.py` (standard library only, Python ≥ 3.8) is the
reference reader and classifies every chunk exactly as `kf-telemetry` does.

Conventions: all integers little-endian; `f64` is IEEE-754 binary64
little-endian; offsets are bytes from the start of the file, chunk or record;
times are `i64` nanoseconds. Kind names (`input_motion`, …) are used in JSON,
dumps and table names.

## 1. File header (16 bytes)

| Offset | Type | Value |
|---|---|---|
| 0 | 8 bytes | magic `4B 46 4C 4F 47 0D 0A 1A` (`"KFLOG\r\n\x1a"`) |
| 8 | u32 | `format_version` = 1 |
| 12 | u32 | reserved = 0 |

## 2. Chunks

Chunks follow the header back to back. Stride = 24 + `payload_len`; since
`payload_len` is a multiple of 8, every chunk starts 8-byte aligned.

| Offset | Type | Field |
|---|---|---|
| 0 | u32 | `payload_len`: multiple of 8, ≤ 1 MiB (1 048 576) |
| 4 | u8 | `kind` (§4) |
| 5 | u8 | `kind_version`: 1 for every v1 kind |
| 6 | u16 | `flags`: bit 0 = payload LZ4-compressed (reserved; v1 writers write 0) |
| 8 | u32 | `record_count` (rules per layout, §4) |
| 12 | u32 | reserved = 0 |
| 16 | `payload_len` bytes | payload |
| 16 + `payload_len` | u32 | CRC-32 (IEEE; Python `zlib.crc32`) over the 16 header bytes followed by the payload |
| 20 + `payload_len` | 4 bytes | zero |

**Reading** checks, in this order, and stops at the first failure (the name
is the dump's stop reason, §9; `offset` is where the chunk starts):

1. File header: fewer than 16 bytes → `truncated` at 0 if the bytes read are
   the magic (≥ 8 bytes) or all zero (an empty file included), else
   `bad_magic`; wrong magic → `bad_magic`; `format_version` ≠ 1 →
   `unsupported_version`; reserved ≠ 0 → `non_zero_reserved` at 12. Nothing
   can be read from such a file.
2. Per chunk: no byte left → clean end of file; 1–15 header bytes →
   `truncated`; `payload_len` > 1 MiB → `payload_too_large`; not a multiple
   of 8 → `payload_misaligned` (both checked before anything is allocated or
   read); payload or the 8 trailer bytes short → `truncated`; CRC differs →
   `crc_mismatch`; trailer bytes 4–7 not zero → `non_zero_trailer`; header
   reserved ≠ 0 → `non_zero_reserved`.

Everything before the failing chunk stays readable. A read error of the
underlying stream stops reading with reason `io`.

## 3. Decoding a CRC-valid chunk

- `kind` unknown, or `kind_version` ≠ 1, or `flags` ≠ 0 → **skipped**: the
  payload is ignored (skipping by `payload_len`) and reported. Future kinds,
  layout versions and compression arrive this way.
- Otherwise the payload is decoded by the kind's layout (§4–§6). If it breaks
  any rule there, the chunk is **undecodable**: reported, its records unused.
  No rule is relaxed for a "mostly fine" payload.

## 4. Kinds

| Code | Name | Layout | Record size | `record_count` |
|---|---|---|---|---|
| 0x00 | `session_header` | JSON | — | 1 |
| 0x01 | `input_motion` | fixed | 32 | n |
| 0x02 | `input_button` | fixed | 24 | n |
| 0x03 | `input_wheel` | fixed | 32 | n |
| 0x04 | `source_declare` | JSON | — | 1 |
| 0x05 | `device_info` | JSON | — | 1 |
| 0x06 | `render_state` | JSON | — | 1 |
| 0x10 | `frame` | fixed | 40 | n |
| 0x11 | `drain` | fixed | 16 | n |
| 0x12 | `frame_timing` | fixed | 32 | n |
| 0x20 | `sim_camera` | fixed | 40 | n |
| 0x21 | `sim_target` | fixed | 72 | n |
| 0x22 | `sim_shot` | fixed | 56 | n |
| 0x23 | `sim_control` | fixed | 32 | n |
| 0x30 | `app_result` | JSON | — | 1 |
| 0x7C | `commit` | commit | 32 + 8n | 1 |
| 0x7D | `clock_sync` | fixed | 32 | n |
| 0x7E | `annotation` | annotation | 24 + text, padded to 8 | number of annotations |
| 0x7F | `session_end` | JSON | — | 1 |

Declaration kinds: `session_header`, `source_declare`, `device_info`.

## 5. Fixed and self-framed layouts

**Fixed kinds:** `payload_len` = `record_count` × record size exactly (0 and 0
is a valid, empty chunk); records are packed with no gaps.

Notation: `offset: type field`. A field marked † is **null** when it holds its
type's sentinel: `i64::MIN` (−9 223 372 036 854 775 808), `u64::MAX`,
`u32::MAX` (4 294 967 295), `u16` 0xFFFF. Sentinels are never used in
arithmetic. `pad` bytes must be zero, or the chunk is undecodable.

| Kind | Layout |
|---|---|
| 0x01 `input_motion` (32) | 0: i64 `t_ns` · 8: i64 `src_ts_ns`† · 16: i32 `dx` · 20: i32 `dy` · 24: u32 `seq` · 28: u16 `source` · 30: u16 `flags` |
| 0x02 `input_button` (24) | 0: i64 `t_ns` · 8: i64 `src_ts_ns`† · 16: u32 `seq` · 20: u16 `source` · 22: u8 `button` · 23: u8 `state` |
| 0x03 `input_wheel` (32) | 0: i64 `t_ns` · 8: i64 `src_ts_ns`† · 16: i32 `dv` · 20: i32 `dh` · 24: u32 `seq` · 28: u16 `source` · 30: u16 `flags` |
| 0x10 `frame` (40) | 0: u64 `frame` · 8: i64 `t_start_ns` · 16: i64 `t_input_cutoff_ns` · 24: u64 `first_tick` · 32: u32 `ticks` · 36: u16 `flags` · 38: u16 pad |
| 0x11 `drain` (16) | 0: u64 `frame` · 8: u16 `source` · 10: u16 pad · 12: u32 `last_seq` |
| 0x12 `frame_timing` (32) | 0: u64 `frame` · 8: i64 `t_submit_ns` · 16: i64 `t_present_ns`† · 24: u32 `cpu_ns` · 28: u32 `gpu_ns`† |
| 0x20 `sim_camera` (40) | 0: u64 `tick` · 8: i64 `yaw_units` · 16: i64 `pitch_units` · 24: f64 `yaw_rad` · 32: f64 `pitch_rad` |
| 0x21 `sim_target` (72) | 0: u64 `tick` · 8: u32 `target` · 12: u8 `event` · 13: u8 `shape` · 14: u16 pad · 16: f64×3 `pos_m` · 40: f64×3 `vel_m_s` · 64: f64 `radius_m` |
| 0x22 `sim_shot` (56) | 0: u64 `tick` · 8: i64 `t_ns` · 16: u32 `target`† · 20: u32 `seq` · 24: f64 `ang_err_rad` · 32: f64 `d_yaw_rad` · 40: f64 `d_pitch_rad` · 48: u8 `hit` (0 or 1, anything else is undecodable) · 49: 7 bytes pad |
| 0x23 `sim_control` (32) | 0: u64 `tick` · 8: i64 `t_ns` · 16: u64 `frame` · 24: u8 `action` · 25: u8 `phase` · 26: u16 `reason` · 28: u16 `new_source`† · 30: u16 pad |
| 0x7D `clock_sync` (32) | 0: i64 `t_ns` · 8: u16 `domain` · 10: 6 bytes pad · 16: i64 `src_ts_ns` (no sentinel) · 24: i64 `uncertainty_ns` |

`seq` is one u32 counter per source shared by motion, button and wheel
records; it wraps, and readers unwrap it (nearest to previous + 1). NaN and
±infinity are invalid in every `f64` (writers refuse them, the validator
reports them; a reader still decodes them). Numeric enum values (`flags`, `button`, `state`, `event`, `shape`,
`action`, `phase`, `reason`, annotation `code`) are listed in
DATA_SPECIFICATION.md §6; unknown values are kept, never dropped.

**`commit` (0x7C):** `record_count` = 1, `payload_len` ≥ 32 and
= 32 + 8 × `n` exactly.

| Offset | Type | Field |
|---|---|---|
| 0 | i64 | `t_ns`: session time of the cut |
| 8 | u64 | `tick`†: last tick of the last complete frame written so far |
| 16 | u64 | `frame`†: that frame (both null before any frame) |
| 24 | u32 | `n` |
| 28 | u32 | pad |
| 32 + 8i | u16, u16 pad, u32 | `source`, `last_seq`: per source, the last `seq` written so far, ascending by source |

**`annotation` (0x7E):** the payload is a sequence of self-framed records
filling it exactly; `record_count` must equal their number.

| Offset | Type | Field |
|---|---|---|
| 0 | i64 | `t_ns` |
| 8 | u16 | `code` |
| 10 | u16 | `source`† |
| 12 | u32 | `first_seq` |
| 16 | u32 | `lost`† (unknown) |
| 20 | u32 | `len` |
| 24 | `len` bytes | `text`, UTF-8 (invalid UTF-8 is undecodable) |
| 24 + `len` | 0–7 bytes | zero pad to the next multiple of 8 |

Fewer than 24 bytes left for a record header, or text plus padding running
past the payload, is undecodable. Gap annotations (`code` 0x0001–0x00FF) say
`lost` seqs of `source` starting at `first_seq` never arrived.

## 6. JSON kinds

The payload is exactly one UTF-8 JSON document, an **object**, padded with
spaces (0x20) to a multiple of 8; `record_count` = 1; ≤ 1 MiB. Writers emit
compact JSON with object keys in byte order. A reader accepts what
`serde_json` accepts, and nothing more:

- RFC 8259 syntax; JSON whitespace (space, tab, LF, CR) around the value; no
  BOM, no trailing data.
- `NaN`, `Infinity`, `-Infinity` are rejected, and so is any number that
  overflows `f64` (`1e400`); underflow gives 0.
- Integers in [−2⁶³, 2⁶⁴) are integers; every other number, and `-0`, is an
  `f64` (correctly rounded).
- Strings must be valid Unicode: a `\u` escape of a lone surrogate is rejected.
- Nesting depth ≤ 127 (the outermost object counts as 1).
- A repeated key: the last value wins.

Unknown fields are preserved in the log and ignored by readers. Typed fields
(checked by the validator; a dump passes the object through unchanged):

**`session_header` (0x00)**, the first chunk of the file:
`schema` (string, `kineforge.session/0.*`), `session_id` (string, UUID
(8-4-4-4-12 hex; KineForge writes v4)), `started_at_utc` (optional string,
RFC 3339 UTC: `YYYY-MM-DDTHH:MM:SS[.fraction]Z`), `clock_domains` (array),
`simulation` (optional), `sensitivity` (optional; required with
`simulation`, model `linear`), plus any other fields (`app`, `source_app`,
`platform`, `scenario`, `experiment`, …).

- clock domain: `id` u16, `kind` (`session`, `linux_monotonic`,
  `linux_realtime`, `linux_boottime`, `x_server_ms`, `wayland_utime`,
  `gameinput_us`, `mach_absolute`, `browser_perf`), `coi` (optional bool),
  `meaning` (`session_clock`, `kernel_flush`, `kernel_event`, `os_dispatch`,
  `compositor`, `browser_receipt`), `resolution_ns`, `offset_to_session_ns`,
  `offset_uncertainty_ns` (i64), `drift_ppm` (f64).
- `simulation`: `sim_hz` (u32 dividing 10⁹), `input_source` (u16, the
  initial driving source), plus other fields (`rng`, …).
- `sensitivity`: `model` (string), `yaw_rad_per_unit`, `pitch_rad_per_unit`,
  `sens_x`, `sens_y` (f64), `invert_y` (bool), plus other fields (`fov`, …).
- Read by replay (`kf-cli replay`), not checked by the validator:
  `simulation.rng` = `{"algorithm": "xoshiro256++", "seeding": "splitmix64",
  "seed": "0x…"}` (hex u64), `sensitivity.directional_gain` (`null` or
  `{"profile": <56 hex digits>, "engine": …, "policy": "carry_capped"}`),
  `scenario` = `{"id", "version", "config"}` with every config field present
  and no others (a scenario config is closed and versioned by its
  `version`: unlike other JSON objects, unknown fields are refused),
  `scoring` = `{"model", "scoring_version"}` (DATA_SPECIFICATION.md §7).

**`source_declare` (0x04):** `id` (u16, 0–0xFFFE), `source_rank` (optional
u32, default `id`), `backend` (string), `input_level` (`device_report`,
`os_raw_counts`, `os_unaccelerated`, `os_absolute_pointer`, `os_pointer`,
`browser_locked_unadjusted`, `browser_locked`, `browser_unlocked`,
`synthetic`, `replayed`), `units` (`counts`, `os_units`, `logical_px`,
`dip`, `css_px`, `points`, `abs_65535`), `value_scale` (1, 256 or 1024),
`clock_domain` (u16, declared in the header), `receipt_meaning`
(`app_dequeue`, `dom_callback`, `imported`), `evidence_class` (`observed`,
`app_reported`, `inferred`, `simulated_truth`, `derived`), `processing`
(array of strings, default empty), plus other fields.

**`device_info` (0x05), `render_state` (0x06), `app_result` (0x30):** any
object (contents in DATA_SPECIFICATION.md §6).

**`session_end` (0x7F):** `completed` (bool), `end_reason` (string:
`completed`, `input_level_changed`, `storage_error`, `recovered`, or
app-specific), `t_end_ns` (i64), `counts` (object: kind name → u64, the
records written per kind excluding `commit` and `session_end`, no zero
entries), `dropped_chunks`, `max_writer_backlog_bytes`, `interruptions`
(u64, default 0), `refresh_hz_measured`, `raf_hz_measured` (optional f64,
omitted when unknown), plus other fields.

## 7. Groups, commits, the terminal `session_end`

A writer appends whole **groups**. Group order: declaration chunks first
(`session_header` first in the first group, then `source_declare` and
`device_info` in arrival order), then every other kind in ascending kind
code (a kind may span several chunks, each ≤ 1 MiB, records in arrival
order), then exactly one `commit` chunk, always last. The file ends with the final
group (which has its commit even when it holds nothing else) and then one
`session_end` chunk, the **terminal chunk**.

Reading uses **committed records only**:

- Chunks are collected until a CRC-valid, decodable `commit`; then they form
  a group with that commit. A commit that is skipped or undecodable is an
  ordinary chunk, not a group boundary.
- A decodable `session_end` that directly follows a commit (no chunk between
  them, at least one commit before it) is the terminal `session_end`; the
  session is **complete**. CRC-valid chunks after it are counted as
  "after end" and never used.
- A `session_end` anywhere else is an ordinary chunk of the group it falls in.
- Chunks after the last commit are **uncommitted**: counted, never used.
- Skipped and undecodable chunks inside a group are reported in place and
  contribute no records.

## 8. Recovery and `raw.recovered.json`

Recovery never modifies `raw.kflog`. It applies only to a file with no
terminal `session_end` and at least one group; such a file is read as
above, and:

- `valid_prefix_bytes` = offset just after the last commit chunk read
  before reading stopped (an error, or the end of the file).
- The **synthetic end** is a framed `session_end` chunk (kind 0x7F,
  `kind_version` 1, `flags` 0, `record_count` 1) whose payload is the compact
  JSON object below, keys in byte order, no spaces, integers in decimal,
  padded with spaces to a multiple of 8:
  `{"completed":false,"counts":{…},"dropped_chunks":0,"end_reason":"recovered","interruptions":0,"max_writer_backlog_bytes":0,"t_end_ns":T}`
  where `T` is the last commit's `t_ns` and `counts` maps each kind name
  (keys in byte order) to the number of committed records of that kind
  (skipped and undecodable chunks and commits not counted; no zero entries).
  The bytes are fully determined by the prefix.
- The **recovered byte stream** is the first `valid_prefix_bytes` of
  `raw.kflog` followed by the synthetic end chunk. It reads as a complete
  session.
- The sidecar `raw.recovered.json` next to `raw.kflog` is a JSON object:
  `valid_prefix_bytes` (u64), `synthetic_end_b64` (the synthetic end chunk,
  standard base64 with padding, RFC 4648 §4; non-canonical encodings are
  invalid), `recovered_sha256` (lower-case hex SHA-256 of the recovered byte
  stream), `tail_bytes_ignored` (u64, file length − `valid_prefix_bytes`).
  Unknown fields are ignored.
- The **session identity hash** is SHA-256 of `raw.kflog` for a complete
  session (all bytes, including any after the terminal chunk), or
  `recovered_sha256` for a recovered one.
- A file that has a terminal `session_end` is complete and any sidecar is
  ignored. A file without a group (or with an unusable file header) cannot
  be recovered.

## 9. Canonical dump (JSON lines)

`kf_telemetry::dump` and `read_kflog.py --jsonl` print the same objects, one
JSON object per line, in file order. Rust prints compact JSON with keys in
byte order; numbers may be formatted differently by other printers
(`1e-7` vs `1e-07`), so compare dumps as parsed values.

- Each record of a committed group: `{"kind": <name>, <field>: <value>, …}`
  with the field names of §5; † sentinels and non-finite `f64` are `null`;
  `sim_target` has arrays `pos_m`, `vel_m_s`; `sim_shot.hit` is a boolean;
  `commit` has `"sources": [[source, last_seq], …]`; `annotation` has
  `text`. JSON kinds: `{"kind": <name>, "value": <object>}`.
- A skipped chunk: `{"kind":"skipped","offset","kind_code","kind_version","flags","record_count","payload_len"}`.
- An undecodable chunk: `{"kind":"undecodable","offset","kind_code"}`.
- Per group: its chunks' lines, then its `commit` line.
- The terminal end, if any: `{"kind":"session_end","value": <object>}`.
- Last: `{"kind":"ending","complete","committed_end","valid_end","uncommitted_chunks","after_end_chunks","stop"}`:
  `committed_end` = offset after the last commit (16 if none), `valid_end` =
  offset after the last CRC-valid chunk, `stop` = `null` at a clean end of
  file or `{"reason","offset"}` with a reason from §2 (`offset` is `null`
  for `io`, `bad_magic`, `unsupported_version`). If the file header is
  unusable the dump is the single line
  `{"kind":"ending","complete":false,"stop":{…}}`.

## 10. Validation (summary of `kf_telemetry::validate`)

A recovered session is validated as its recovered byte stream. Errors:
reading stopped early; first chunk not `session_header` (or a second one,
or one this reader skips); header or `source_declare` not matching §6
(schema prefix, `session_id` a UUID (8-4-4-4-12 hex), `started_at_utc`
`YYYY-MM-DDTHH:MM:SS[.fraction]Z` with a real date and time, `sim_hz`
divides 10⁹, `simulation` needs a `linear` sensitivity, source id ≠ 0xFFFF
and declared once, `value_scale` 1/256/1024, `clock_domain` declared); a
declaration after a non-declaration chunk or non-declaration kind codes
decreasing within a group; undecodable chunks; NaN or ±infinity; input records, drains,
annotations (`source`) or `clock_sync` (`domain`) naming undeclared ids;
per source in unwrapped `seq` order, a repeated or decreasing `seq` or a
decreasing `t_ns`; commits whose `t_ns`, `tick` or `frame` go backwards,
whose `last_seq` differs from the last seq read, that list a source twice
or omit one with records; with `simulation`: frame numbers not increasing,
`first_tick` ≠ previous `first_tick + ticks`, a drain of a source other than
the driving source or a second drain of a frame, `switch_source` outside a
pause, without `new_source`, or to a source with a different
`input_level`/`units`/`value_scale` (or undeclared), `sim_camera` radians
not bit-equal to the recomputation from units; no terminal `session_end`;
its `counts` differing from the committed records or `t_end_ns` before the
last commit; chunks after it. Warnings: skipped chunks; uncommitted chunks;
`seq` gaps not covered by a gap annotation of that source (capture bug; an
annotation with unknown `lost` covers only the gap starting at its
`first_seq`); unknown `sim_control.action`, `sim_target.event` or `shape`.
The report keeps the first 1000 issues and counts the rest; drains and
`sim_control` are checked in replay order (§9.1 of DATA_SPECIFICATION.md:
phase 0, drain, phase 1 per frame), not file order.

## 11. Session bundle and `.kfz`

On disk a session is a directory `sessions/<session-id>/` holding
`raw.kflog` (never modified after close), `raw.recovered.json` (only after
recovery), `manifest.json` (convenience copy of header, declarations (up to
256 KiB of their JSON; `declarations_omitted` counts the rest), end record
and file hashes; regenerable) and derived Parquet tables (re-encoded
committed records; regenerable, not needed to read a session).

A `.kfz` export is a ZIP archive:

- every member STORED (no compression), unencrypted, compressed size =
  size; local and central headers agree; KineForge writes no ZIP64, so each
  member is < 4 GiB;
- flat member names, each exactly one of `raw.kflog` (required),
  `manifest.json`, `FORMAT.md` (this file), or `<table>.parquet` with
  `<table>` ∈ `input_motion`, `input_events`, `frames`, `frame_timing`,
  `drains`, `sim_camera`, `sim_targets`, `sim_shots`, `sim_control`,
  `annotations`, `app_results`, `clock_sync`; no directories, paths or
  duplicates;
- a recovered session is exported with its recovered byte stream as
  `raw.kflog` and no sidecar;
- KineForge stamps every member 1980-01-01 00:00 with a Unix "made by", so
  the same members in the same order give identical bytes.

An importer checks all of this, and every declared member size and their
total against its limits, before reading any member data.

## 12. Reference reader

`tools/read_kflog.py [--jsonl] [--recovered SIDECAR] FILE` (library:
`records(f)`, `recovered(raw, sidecar)`). Bundles never contain code; use
the published copy and check its hash:

read_kflog.py SHA-256: f560b0f4799f2bdffbf6ebed25c169f4ffa4ea15d54ee018aa3a201ded0b03cf
