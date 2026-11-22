# KineForge research: Rust math/simulation, determinism, telemetry formats, storage, name check

Accessed: **2026-11-13** for every source below, unless a row says otherwise. Crate data comes from the `crates.io/api/v1/crates/<name>` JSON (`max_stable_version`, `updated_at`, `license`, `repository`, `rust_version`, `recent_downloads` = last 90 days). GitHub data comes from `gh api`. "UNVERIFIED" means I did not confirm it against a primary source.
Toolchain context: the stable std docs header reads `std 1.100.0 (0ef1bc386 2026-11-09)` ([f32 docs](https://doc.rust-lang.org/stable/std/primitive.f32.html)). No `rustc` is installed on this machine, so nothing was compiled or run. Every claim about codegen comes from docs, specs, or crate source read from the published `.crate` tarballs.

---

## 0. Decisions in one screen

| Topic | Decision | Why (evidence below) |
|---|---|---|
| Vector math | **glam 0.34.3**. In the deterministic core use only the **f64 types (`DVec3`/`DQuat`/`DMat4`)** with the glam `libm` feature. Use f32 SIMD types only in the renderer. | In the glam 0.34.3 source the f32 SIMD backends sum dot products in different orders (SSE2 `(x+z)+(y+w)`, NEON `vaddvq`, scalar `((x+y)+z)+w`), and the SSE2 path uses FMA when `target_feature="fma"`. The f64 types have only a scalar backend. |
| Collision | **Hand-write** ray–sphere, ray–capsule and ray–AABB/OBB, about 100 lines. Use no parry/rapier. | An aim trainer only needs hitscan against primitives. These tests need only `+ - * / sqrt`, which are bit-exact on every target. parry3d 0.31.1 is the fallback if mesh hitboxes ever appear. |
| RNG | **Own the PRNG and the samplers.** Pin one named algorithm: xoshiro256++ / PCG64 (rand_xoshiro, rand_pcg) or about 15 lines in-house. Never use `StdRng`/`SmallRng`, `gen_range`, or `shuffle` in scored or replayed paths. | `StdRng` and `SmallRng` opt out of all reproducibility guarantees. rand 0.9 broke `Uniform`, `shuffle`, `choose` and float sampling values. rand 0.10 swapped `rand_chacha` for `chacha20`. |
| Transcendentals | Call the **`libm` crate (pinned)** everywhere in the core. Never call `f64::sin`, `atan2`, `powf` or similar there. Ban them with clippy `disallowed_methods`, together with `algebraic_*` and `mul_add_relaxed`. | std documents their precision as non-deterministic: it "varies by platform, Rust version, and can even differ within the same execution". |
| Float type | Use **f64** for simulation, scoring and accumulation, and f32 for GPU. Accumulate mouse counts as **i64 integers** and derive angles from the total. | Avoids drift. Fixed-point (`fixed` 1.31) is unnecessary for v1. |
| Raw telemetry | A **custom append-only chunked log**: fixed-layout little-endian records, a length prefix and **CRC-32 (IEEE)** per chunk. Use `std` plus `crc32fast` only. Natively, write ≤100 ms chunks and fsync ≤1 s. In the browser, use an OPFS `FileSystemSyncAccessHandle` in a worker. | Throughput is trivial for every format (see B3), so crash recovery, total ordering across event kinds, and a stdlib-only Python reader decide it. |
| Derived tables | **Parquet** (`parquet` 60.1.0) written at session close or during crash recovery. Use zstd natively. On wasm use snappy, lz4_raw or gzip (all pure Rust), or zstd if the wasm build has clang. | Every analysis tool reads it: pyarrow, polars, DuckDB, R arrow, hyparquet/parquet-wasm in JS. |
| Manifest | **JSON** (`serde_json`). It holds the versions, seed, RNG algorithm, config, clocks, file SHA-256 hashes and a `completed` flag. | Human-readable, and every language parses it. |
| Export bundle | **ZIP with members stored uncompressed** containing manifest.json, the raw log, derived/*.parquet, FORMAT.md and a stdlib-only `read_kflog.py`. | ZIP is a Library of Congress "preferred" aggregation format, and Python reads it with stdlib `zipfile`. |
| SQLite | **Leave it out of v1.** Add rusqlite (native) plus sqlite-wasm-rs with OPFS `sahpool` (web) only if cross-session in-app queries outgrow scanning manifests. | It is a C dependency on both targets, and the web build is Worker-only and single-connection. |
| bincode | **Do not use it.** | It is unmaintained (RUSTSEC-2025-0141). Version 3.0.0 is a tombstone release whose lib.rs is only `compile_error!`. |
| Name | "KineForge" is **free on crates.io, npm and PyPI, and no GitHub user or org of that name exists**. Several small 2026 projects use "Kineforge", including a Product Hunt robotics-AI launch. kineforge.com is registered but has no website. **Medium risk.** USPTO and EUIPO were not reachable programmatically. | See D. |

---

## A. Math / simulation

### A1. Crate status (crates.io API, 2026-11-13)

| Crate | Latest stable | Last publish | License | MSRV | 90-day dl | Notes |
|---|---|---|---|---|---|---|
| glam | 0.34.3 | 2026-11-08 | MIT OR Apache-2.0 | 1.85 | 65.6M | Features: `scalar-math`, `libm`, `nostd-libm`, `core-simd` (nightly), `mint-05`, `rand-010`, `serde`, `rkyv-08`, `f64` and others. README: "SIMD on wasm32/wasm64 passes tests but has not been benchmarked". |
| nalgebra | 0.35.0 | 2026-05-24 | Apache-2.0 | 1.89 | 18.3M | Features `libm` and `libm-force` (via simba). glam conversion features stop at `convert-glam033`, so there is **no bridge for glam 0.34**. Heavy on generics. |
| ultraviolet | 0.10.0 | 2025-04-26 | MIT OR Apache-2.0 OR Zlib | – | 349k | Repo last pushed 2026-10-04. Its niche is wide/SoA SIMD. Not needed. |
| mint | 0.5.9 | 2022-02-02 | MIT | – | 1.96M | Repo last pushed 2024-01. Stable and frozen interop types. glam's `mint-05` covers interop if it is ever needed. |
| parry3d | 0.31.1 | 2026-09-18 | Apache-2.0 | – | 1.1M | **Now built on glam via `glamx` 0.3** (not nalgebra). `enhanced-determinism` enables `glamx/scalar-math`, `glamx/libm` and `simba/libm_force`, which independently confirms that glam SIMD and std libm are the non-deterministic parts. |
| rapier3d | 0.36.1 | 2026-10-10 | Apache-2.0 | 1.86 | 635k | Its `enhanced-determinism` feature is incompatible with `simd8`. Docs: cross-platform determinism holds if the platforms "strictly comply to the IEEE 754-2008", including wasm ([rapier determinism docs](https://rapier.rs/docs/user_guides/rust/determinism)). |
| rand | 0.10.3 | 2026-09-20 | MIT OR Apache-2.0 | 1.85 | 636M | See A4. |
| rand_core | 0.10.1 | 2026-09-02 | MIT/Apache-2.0 | – | – | `RngCore` was renamed to `Rng` (rand 0.10 CHANGELOG). |
| rand_chacha | 0.10.0 | 2026-02-02 | MIT OR Apache-2.0 | 1.85 | 407M | rand itself now uses `chacha20` 0.10.2 (RustCrypto) instead. |
| rand_pcg | 0.10.2 | 2026-04-11 | MIT OR Apache-2.0 | 1.85 | 118M | repo rust-random/rngs |
| rand_xoshiro | 0.8.1 | 2026-05-16 | MIT OR Apache-2.0 | 1.85 | 46.7M | rand 0.10 also re-exports `Xoshiro128PlusPlus` and `Xoshiro256PlusPlus`. |
| statrs | 0.19.1 | 2026-08-11 | MIT | 1.89 | 12.6M | Default features pull in **nalgebra 0.35 + rand 0.10**. Use `default-features=false` if you only need distributions. |
| argmin | 0.11.0 | 2025-09-28 | MIT OR Apache-2.0 | – | 1.6M | Depends on **rand 0.9 + rand_xoshiro 0.7**, which duplicates rand in the tree. Has a `wasm-bindgen` feature for getrandom. |
| levenberg-marquardt | 0.15.0 | 2025-08-03 | MIT | – | 445k | Depends on **nalgebra ^0.34**, so using it with statrs gives two nalgebra versions. |
| biquad | 0.6.0 | 2026-03-22 | MIT OR Apache-2.0 | – | 172k | Only dependency is num-traits. Simple DF1/DF2T biquads. |
| idsp | 0.23.1 | 2026-10-17 | MIT OR Apache-2.0 | 1.91 | 15k | quartiq (Stabilizer). Fixed-point and float DSP, with an optional `py` (pyo3) feature. |
| rustfft | 6.4.1 | 2025-09-18 | MIT OR Apache-2.0 | 1.61 | 11.8M | Pure Rust with avx/sse/neon/`wasm_simd` paths. Its results may differ by backend (UNVERIFIED), so keep it for analysis only. |
| rubato | 5.1.0 | 2026-11-12 | MIT OR Apache-2.0 | – | 6.3M | Audio-oriented resampler, overkill for mouse data. |
| sci-rs | 0.4.1 | 2024-11-22 | MIT OR Apache-2.0 | – | 61k | Stale. A SciPy-like filter design library. |
| savgol-rs / staged-sg-filter | 0.1.0 / 0.2.3 | 2023 / 2024 | – | – | <400 | Small and stale. A Savitzky–Golay filter is a fixed-coefficient convolution, so **hand-roll it** (coefficients computed once offline or with a least-squares solve). |
| splines | 5.0.0 | 2025-04-26 | BSD-3-Clause | 1.86 | 164k | Catmull-Rom, cubic Hermite and Bézier. Hosted on sourcehut. |
| enterpolation | 0.3.0 | 2025-05-08 | MIT OR Apache-2.0 | – | 32k | Alternative interpolation crate. |
| fixed | 1.31.0 | 2026-03-20 | MIT/Apache-2.0 | **1.93** | 3.5M | Deterministic by construction, but has no trig. |
| libm | 0.2.16 | 2026-03-17 | MIT | 1.63 | 172M | Lives in the rust-lang/compiler-builtins repo. Port of musl libm. See A6. |

No crate for the "one euro" filter turned up in a crates.io search (`q=one euro`, `1euro`). That filter is about 20 lines, so hand-roll it.

### A2. glam determinism, read from the 0.34.3 source tarball

- `src/sse2.rs` `dot4_into_m128`: "keeping the (x + z) + (y + w) grouping".
- `src/neon.rs` `dot4`: `vaddvq_f32` horizontal add.
- `src/f32/scalar/vec4.rs` `dot`: `(x*x)+(y*y)+(z*z)+(w*w)`, evaluated left to right.
- These are **different summation orders, so `Vec4`/`Vec3A`/`Quat`/`Mat4` results can differ in the last ulp between x86_64, aarch64, wasm `simd128` and scalar builds.**
- `src/sse2.rs` `m128_mul_add`: "Use a fused multiply-add whenever the target supports it … may produce different results to targets without FMA". Building with `-C target-cpu=native` or `x86-64-v3` therefore changes results.
- No approximate `rsqrt`/`rcp` intrinsics appear anywhere (grep found none).
- `glam::f64` (`DVec*`, `DQuat`, `DMat*`, `DAffine*`) has **no SSE2/NEON/wasm backend directories**, so it is scalar-only. `DVec3::dot` is `(x*x)+(y*y)+(z*z)`.
- glam's math backend uses `libm::*` when the `libm` feature is on; otherwise it uses `std` (the `f32::sin` family). Inside glam, `mul_add` maps to `libm::fmaf` or `f32::mul_add`, and both are correctly rounded and therefore deterministic.
- `acos_approx` is glam's own polynomial (DirectXMath port), so it is deterministic.
- **Cargo feature unification:** enabling `glam/scalar-math` or `glam/libm` anywhere in a build turns it on for every glam user in that binary, the renderer included. If the core uses only `DVec*` you need only `libm`, not `scalar-math`.

### A3. Do we need a collision library?

No, not for primitive hitboxes. Ray vs sphere is one quadratic with one `sqrt`. Ray vs AABB is the slab method (min/max and division). Ray vs capsule is a segment–ray closest-approach calculation plus a sphere test. Ray vs OBB transforms the ray into box space, then runs the slab test.

All of these use only IEEE-exact operations, so they are **bit-reproducible on all targets with no libm at all**. parry3d is worth it only for triangle-mesh hitboxes, broad-phase over thousands of targets, or swept shapes. If adopted, it needs `enhanced-determinism` and still runs scalar.

### A4. RNG reproducibility

The rand book's reproducibility page ([crate-reprod](https://rust-random.github.io/book/crate-reprod.html)) says:

- **StdRng and SmallRng "opt out of all reproducibility guarantees"** and may change output in any release.
- Other items are "expected to be portable" across platforms and patch releases.
- Value-breaking changes are allowed in minor releases, but must be noted in the CHANGELOG.
- Results from `f32`/`f64` distributions "may not be portable" because of transcendental functions.

rand CHANGELOG ([GitHub](https://github.com/rust-random/rand/blob/master/CHANGELOG.md)):

- **0.9.0 (2025-01-27)** had a "Reproducibility-breaking changes" section:
  - `SmallRng::seed_from_u64`, `UniformFloat`, `Slice`, `Uniform<usize>`
  - New algorithms for `shuffle`, `partial_shuffle` and `choose`
  - Canon/Lemire methods for `Uniform`
  - `sample_floyd`
- **0.10.0 (2026-02-08)**:
  - `rand_chacha` was replaced by `chacha20`; `StdRng` output "remains the same".
  - Renames: `Rng` → `RngExt`, `OsRng` → `SysRng`, `choose_multiple` → `sample`.
  - `from_os_rng`, `ReseedingRng`, `Clone` for `StdRng`, and the `small_rng` feature were removed.
  - MSRV is 1.85, edition 2024.
- 0.10.1 fixed a soundness bug. 0.10.2 and 0.10.3 fixed bugs in `Uniform<char>`, `WeightedIndex` and inclusive float ranges.

What this means:

- The **generator** algorithms (xoshiro256++, PCG64, ChaCha) are fixed specifications. Their raw `u64` stream is stable for good, given an equal seed and an equal seeding procedure.
- **Distributions** (`random_range`, floats in [0,1), `shuffle`) are where values have broken between versions.

Recipe:

1. Store `{algorithm: "xoshiro256++", seed: u64 or [u8;32], seeding: "splitmix64"}` in the manifest.
2. Implement integer ranges yourself, for example Lemire's method with rejection, written down and frozen.
3. Get floats as `(u64 >> 11) as f64 * 2^-53`.
4. Shuffle with Fisher–Yates using your own range function.
5. Commit golden vectors (first N outputs per seed) as a test.

That is about 40 lines and removes rand from the core entirely. If rand is used, use it only for `RngCore::next_u64` from a named generator crate.

### A5. Floating-point determinism facts

| Topic | Fact | Source |
|---|---|---|
| Exactly specified ops | `+ - * / %`, `abs`, `copysign`, **`mul_add`**, **`sqrt`**, float `as` casts and comparisons "exactly match IEEE 754-2008" (roundTiesToEven, no FTZ, no traps). | [RFC 3514 float semantics](https://rust-lang.github.io/rfcs/3514-float-semantics.html) |
| mul_add / sqrt | Docs: "guaranteed to be the rounded infinite-precision result … guaranteed not to change". `floor`/`ceil`/`round`/`round_ties_even`/`trunc`: "always returns the precise result". | [f64 docs](https://doc.rust-lang.org/std/primitive.f64.html) |
| Transcendentals | `sin`, `cos`, `tan`, `exp`, `ln`, `powf`, `powi`, `cbrt`, `hypot`, `atan2` and the rest: "The precision of this function is non-deterministic. This means it varies by platform, Rust version, and can even differ within the same execution". Several are documented as calling **libc** on Unix and Windows. | [f64 docs](https://doc.rust-lang.org/std/primitive.f64.html) |
| FMA contraction | rustc does not fuse `a*b+c` implicitly. RFC 3514 requires `*` and `+` each to match IEEE exactly, which rules out contraction. Fusion exists only as an explicit opt-in: `std::intrinsics::fmuladd` ("non-deterministically executing either a fused multiply-add or two operations"), stabilized as `mul_add_relaxed` per the intrinsic docs. I could not confirm `mul_add_relaxed` on the stable f32 page, so its stability is UNVERIFIED. The **`algebraic_add`/`algebraic_mul`/… methods appear on the stable f32 page** and are documented as "non-deterministic". No primary statement saying "rustc never sets the LLVM `contract` flag" was found, so that wording is UNVERIFIED, but it follows from RFC 3514's guarantees. | [RFC 3514](https://rust-lang.github.io/rfcs/3514-float-semantics.html), [fmuladd](https://doc.rust-lang.org/nightly/std/intrinsics/fn.fmuladd.html), [f32 docs](https://doc.rust-lang.org/stable/std/primitive.f32.html) |
| x87 | i586 (no SSE2) gives "results that differ from IEEE 754-2008 even outside of NaNs" (rust#114479). On 32-bit x86 generally, x87 return values can change NaN payloads (rust#115567). x86_64 always uses SSE2. **Ship no 32-bit x86 builds.** | RFC 3514 |
| NaN bits | The sign and payload of a produced NaN are non-deterministic (from a defined set). The `NAN` constant's bits "may change across Rust versions and target platforms". **Never hash or serialize NaN bit patterns.** Canonicalize, or reject NaN at the boundaries. | RFC 3514, f64 docs |
| wasm | Per RFC 3514, NaN generation on wasm is non-deterministic; everything else is IEEE. Wasm 3.0 (live standard since 2025-09-17) defines an **opt-in deterministic profile** covering NaNs and relaxed SIMD. Browsers are not required to implement it. | [Wasm 3.0 announcement](https://webassembly.org/news/2025-09-17-wasm-3.0/) |
| relaxed-simd | `relaxed_madd` "Computes a * b + c with either one rounding or two roundings", which varies by engine and hardware. An LLVM change lowers `fmuladd` to `relaxed_madd` when relaxed-simd is enabled ([llvm-commits 2025-10](https://lists.llvm.org/pipermail/llvm-commits/Week-of-Mon-20251013/1753482.html)). **Do not enable `+relaxed-simd` for the core.** | [Rust core::arch relaxed_simd docs (mirror)](https://www.rustmax.net/api/core/core_arch/wasm32/relaxed_simd/) |
| libm crate | Pure Rust port of musl. Its arch overrides (`src/math/arch/mod.rs`) replace only `sqrt`, `fma`, `floor`/`ceil`/`trunc`/`rint`/`fabs`, all of which are exactly rounded, with hardware instructions on wasm32, SSE2 and aarch64. Trig, exp and log stay in software, so **results are identical across x86_64, aarch64 and wasm32** apart from NaN payloads. The `force-soft-floats` feature exists. Not correctly rounded (it is musl-accurate), but deterministic. **Pin the exact version** (`=0.2.16`), because an algorithm change in a libm release would change values. | libm-0.2.16.crate source |

### A6. What must be bit-reproducible, and how

| Path | Needs bit-exact? | How |
|---|---|---|
| Input → view angle (yaw/pitch from counts × sens × gain) | **Yes**, because replay and scoring hang off it. | Keep i64 cumulative counts. Compute `angle = (counts_total as f64) * deg_per_count` fresh each tick; never accumulate float increments. To match a specific game's formula (e.g. f32 `m_yaw*sens*counts`), replicate its float type and operation order; IEEE basic ops make that exact. `fixed` is needed only if a target formula is defined in fixed-point. |
| Target spawning / motion | **Yes** | Own PRNG and samplers (A4). Motion as polynomial or piecewise-linear functions of the tick index. Any trig goes through `libm::sin`/`cos`. |
| Hit tests | **Yes** | Hand-written primitives (A3) using f64 basic ops plus `sqrt`. |
| Scores / metrics stored as "official" | **Yes**, or at least recomputable | Only basic ops and `libm`, in a fixed reduction order (sequential, no rayon or SIMD reductions). Put `scoring_version` in the manifest. |
| Time base | **Yes** | Simulation is driven by tick index plus recorded input timestamps (i64 ns), never by wall-clock floats. |
| Rendering, interpolation and FX | No | glam f32 SIMD and the GPU are fine. |
| Offline analysis (filters, FFT, fits, statistics) | No, tolerance-based | statrs, rustfft, argmin, or Python. |

**Enforcement:**

- In the core crate, set clippy `disallowed_methods` for `f32::{sin,cos,tan,asin,acos,atan,atan2,exp,exp2,ln,log*,powf,powi,sinh,…}`, the same for f64, and `algebraic_*`.
- Do not use `-C target-cpu=native` for release builds of the core. With f64 scalar math it would not matter anyway, but it does matter for any f32 glam SIMD that sneaks in.
- Use no `HashMap` iteration in the simulation (RandomState ordering).
- Add a CI golden-replay test: hash the simulation state after N ticks of a recorded input log, and assert the same hash on x86_64-linux, aarch64-macos and wasm32 (wasmtime or node).

---

## B. Telemetry / data formats

### B1. Crate status (crates.io API, 2026-11-13)

| Crate | Version | Last publish | License | MSRV | Notes |
|---|---|---|---|---|---|
| arrow / arrow-ipc / arrow-array | 60.1.0 | 2026-10-28 | Apache-2.0 (arrow-array: Apache-2.0 AND MIT) | 1.88 | arrow-ipc codecs: `lz4` → lz4_flex (pure Rust), `zstd` → zstd (C). CI builds `arrow --no-default-features --features=json,csv,ipc,ffi` for wasm32-unknown-unknown and wasip1. |
| parquet | 60.1.0 | 2026-10-28 | Apache-2.0 | 1.88 | Default features: arrow, snap, brotli, flate2-zlib-rs, lz4, zstd, base64, simdutf8. **CI runs `cargo build -p parquet --target wasm32-unknown-unknown` with default features, after installing clang.** zstd is the C library via zstd-sys, so a wasm build with zstd needs clang that can target wasm. Pure-Rust codecs: snap, lz4_flex (LZ4_RAW), brotli (rust-brotli), flate2 with zlib-rs or the rust_backend. `ruzstd` cannot be plugged into parquet. ([parquet Cargo.toml](https://github.com/apache/arrow-rs/blob/main/parquet/Cargo.toml), [parquet.yml CI](https://github.com/apache/arrow-rs/blob/main/.github/workflows/parquet.yml)) |
| ruzstd | 0.9.1 | 2026-10-11 | MIT | 1.87 | Pure Rust. The decoder is complete but "about 3.5 times slower" than C on enwik9. The encoder supports only Uncompressed and Fastest (≈ level 1); Default, Better and Best are unchecked in its README. |
| zstd | 0.14.4 | 2026-11-12 | BSD-3-Clause | 1.64 | C (zstd-sys). |
| lz4_flex | 0.14.0 | 2026-07-14 | MIT | 1.81 | Pure Rust, block and frame formats. |
| snap | 1.1.2 | 2026-07-15 | BSD-3-Clause | – | Pure Rust Snappy. |
| brotli | 9.1.1 | 2026-10-27 | BSD-3-Clause AND MIT | 1.59 | Pure Rust. |
| flate2 / miniz_oxide | 1.1.11 / 0.9.1 | 2026-11 / 2026-03 | MIT OR Apache-2.0 | – | zlib-rs backend is pure Rust. |
| rusqlite | 0.42.0 | 2026-11-11 | MIT | 1.89 | **Default feature `ffi-sqlite-wasm-rs`: on wasm32-unknown-unknown it uses sqlite-wasm-rs instead of libsqlite3-sys** ([README](https://github.com/rusqlite/rusqlite)). |
| sqlite-wasm-rs / sqlite-wasm-vfs | 0.6.3 / 0.3.0 | 2026-11-10 / 2026-09-20 | MIT | 1.81 | VFSes: `memory` and `sahpool` (OPFS, ported from the official sqlite-wasm). Built with `SQLITE_THREADSAFE=0`. Compiles the SQLite C source (amalgamation), so a C compiler for wasm is presumably required (UNVERIFIED). ([README](https://github.com/Spxg/sqlite-wasm-rs)) |
| serde_json | 1.0.154 | 2026-10-11 | MIT OR Apache-2.0 | 1.71 | |
| rmp-serde | 1.3.1 | 2025-12-23 | MIT | 1.85 | Repo last pushed 2025-12-23. |
| flatbuffers | 25.12.19 | 2025-12-19 | Apache-2.0 | 1.51 | Code generation needs `flatc` (C++). `flatbuffers-build` 0.2.4 bundles it. |
| prost | 0.14.6 | 2026-11-12 | Apache-2.0 | 1.85 | Code generation needs `protoc`, or the pure-Rust `protox` 0.10.0. |
| capnp | 0.27.3 | 2026-10-27 | MIT | 1.81 | Code generation needs the `capnp` tool. |
| postcard | 1.1.3 | 2025-07-24 | MIT OR Apache-2.0 | – | "As of v1.0.0, postcard has a documented and stable wire format" ([spec](https://postcard.jamesmunns.com)). Repo active (pushed 2026-11-06). |
| **bincode** | 3.0.0 | 2025-12-16 | MIT | 1.85 | **Unmaintained.** The 3.0.0 README says: "Due to a doxxing and harassment incident, development on bincode has ceased". 3.0.0's lib.rs is only `compile_error!`. GitHub repo archived (last push 2025-08-15). [RUSTSEC-2025-0141](https://rustsec.org/advisories/RUSTSEC-2025-0141.html) is "informational = unmaintained" and treats 1.3.3 as complete. Suggested alternatives: **wincode** (bincode-compatible, 0.6.3, Apache-2.0, anza-xyz), postcard, bitcode, rkyv. |
| ciborium | 0.2.2 | **2024-01-24** | Apache-2.0 | 1.58 | No release in nearly 3 years. Repo still pushed (2026-10-21), 57 open issues. Alternative: minicbor 2.3.1 (**BlueOak-1.0.0** license). |
| rkyv | 0.8.18 | 2026-08-05 | MIT | 1.81 | Zero-copy, but its archived format is Rust-specific, so it is a poor fit for Python/R. |
| bitcode | 0.6.9 | 2025-12-18 | MIT OR Apache-2.0 | 1.70 | Compact, but no cross-language reader. |
| crc32fast | 1.5.5 | 2026-11-10 | MIT OR Apache-2.0 | 1.63 | CRC-32 IEEE, the same polynomial as Python `zlib.crc32`. |
| zip | 9.5.1 | 2026-11-10 | MIT | – | Features include `deflate-flate2-zlib-rs` and `zstd`. Stored mode needs no codec. |
| sha2 / blake3 | 0.11.1 / 1.8.7 | 2026 | MIT OR Apache-2.0 / CC0 OR Apache-2.0 | – | SHA-256 is in Python `hashlib` and the browser's SubtleCrypto, so prefer it for manifest hashes. |
| polars (Rust) | 0.55.2 | 2026-08-06 | MIT | – | External tool. Python Polars release tags `py-2.0.0` (2026-10-06) and `py-2.1.0` (2026-10-25) are on GitHub. |
| duckdb (Rust) | 1.20000.2 | 2026-11-13 | MIT | 1.88 | DuckDB **v2.0.0** released 2026-10-21 ([release](https://github.com/duckdb/duckdb/releases/tag/v2.0.0)). The crate's version encoding seems to map to DuckDB 2.0 (UNVERIFIED). |

npm, 2026-11-13: `@duckdb/duckdb-wasm` latest tag is **1.33.1-dev57.0 (2026-06-22)**, a dev tag that may lag DuckDB 2.0 (UNVERIFIED). `@sqlite.org/sqlite-wasm` 3.54.1-build1 (2026-10-19). `apache-arrow` 21.2.0. `hyparquet` 1.33.0 (pure JS Parquet reader, 303 KB unpacked). `nodejs-polars` 0.26.2.

### B2. Evaluation matrix

Legend: ✔ good, ~ partial or conditional, ✘ poor.

| Format | Write throughput at our rates | Size (mouse/frame records) | Incremental append | Crash recovery | Schema evolution | Random access for replay | Python/R/JS/DuckDB | Long-term preservation | wasm32 build | Codegen/toolchain |
|---|---|---|---|---|---|---|---|---|---|---|
| **Custom chunked log (LE structs + len + CRC32)** | ✔ | ✔ 16 B/mouse event raw; optional lz4 per chunk | ✔ | ✔ truncate at first bad CRC | ~ per-kind version byte; readers skip unknown kinds by length | ~ needs an index (chunk offsets) or a linear scan | ~ needs a reader; a 40-line stdlib-only Python reader ships in the bundle | ~ document the spec in FORMAT.md | ✔ std only | none |
| Arrow IPC **stream** | ✔ | ~ per-batch flatbuffer metadata means batches need ≥1k rows; lz4/zstd body compression | ✔ (append batches) | ~ readable up to the last complete message; the EOS marker is optional | ✔ but **one schema per stream**, so heterogeneous events need several streams or unions | ~ sequential | ✔ pyarrow, polars `read_ipc_stream`, R arrow/nanoarrow, apache-arrow JS. DuckDB via the **nanoarrow** community extension (`read_arrow`) ([DuckDB blog](https://duckdb.org/2025/05/23/arrow-ipc-support-in-duckdb.html)) | ~ | ✔ (CI-built) | none |
| Arrow IPC **file** | ✔ | same | ✘ footer written at close | ✘ no footer means the file reader fails (the body is stream-readable after the magic) | as above | ✔ footer gives random batch access | ✔ | ~ | ✔ | none |
| **Parquet** | ✔ (batch) | ✔✔ columnar encodings plus zstd | ✘ footer at close; append means a new file or row group | ✘ until closed | ✔ add columns | ✔ row groups and statistics | ✔✔ everything | ✔ open Apache spec, very wide support (not in the LoC preferred list) | ✔ (C zstd needs clang) | none |
| SQLite | ✔ batched transactions | ~ row overhead | ✔ | ✔✔ WAL + `synchronous=NORMAL` "is safe from corruption", but the last commits may roll back on power loss ([pragma docs](https://www.sqlite.org/pragma.html#pragma_synchronous)) | ✔ ALTER TABLE | ✔ indexes | ✔ stdlib `sqlite3`, R DBI, DuckDB `sqlite` extension | ✔✔ **LoC "preferred"** dataset format ([LoC RFS](https://www.loc.gov/preservation/resources/rfs/data.html)) | ~ sqlite-wasm-rs, Worker-only OPFS | C compiler |
| JSON / NDJSON | ✔ | ✘ about 4× postcard (B3) | ✔ NDJSON | ✔ drop the last line | ✔ | ✘ | ✔✔ | ✔ text | ✔ | none |
| MessagePack (rmp-serde) | ✔ | ~ | ✔ with framing | ~ needs framing | ✔ if maps are used | ✘ | ✔ msgpack libraries everywhere | ~ | ✔ | none |
| postcard | ✔ | ✔ varints | ✔ with framing | ~ needs framing | ✘ not self-describing; stable 1.0 wire spec | ✘ | ✘ no maintained Python reader (UNVERIFIED) | ~ | ✔ | none |
| FlatBuffers / Cap'n Proto | ✔ | ~ | ✔ with framing | ~ | ✔ field ids / tables | ✔ zero-copy | ✔ codegen per language | ~ | ✔ | flatc / capnp |
| Protobuf (prost) | ✔ | ~ varints | ✔ with framing | ~ | ✔✔ | ✘ | ✔✔ | ✔ | ✔ | protoc or protox |
| CBOR (ciborium) | ✘ slowest binary option in B3 | ~ | ✔ | ~ | ✔ self-describing | ✘ | ✔ cbor2 / cbor-x | ✔ RFC 8949 | ✔ | none |
| bincode | – | – | – | – | – | – | – | – | – | **unmaintained** |

### B3. Published benchmark numbers (no invented numbers)

Source: [rust_serialization_benchmark](https://github.com/djkoloski/rust_serialization_benchmark) README, "Last updated: 2026-11-05". Setup: rustc 1.101.0-nightly, x86_64, 4-CPU Intel.

The **`mesh`** dataset is the closest analogue to fixed-size float telemetry ("a single mesh … array of triangles, each … three vertices and a normal"; 6.0 MB raw, which by size arithmetic is about 125k triangles of f32 data).

| Crate | Serialize | Deserialize | Size (B) | zstd size (B) |
|---|---|---|---|---|
| wincode 0.6.3 | 408.18 µs | 407.75 µs | 6,000,008 | 5,346,908 |
| postcard 1.1.3 | 411.56 µs | 1.0549 ms | 6,000,003 | 5,346,897 |
| rkyv 0.8.18 | 447.97 µs | 405.36 µs (unvalidated) | 6,000,008 | 5,346,872 |
| flatbuffers 25.12.19 | 618.11 µs | n/a | 6,000,024 | 5,346,878 |
| bitcode 0.6.9 | 1.0012 ms | 533.75 µs | 6,000,006 | 4,921,841 |
| prost 0.14.4 | 3.78 ms (encode) | 9.56 ms (decode) | 8,750,000 | 6,421,877 |
| capnp 0.27.3 | 7.35 ms | n/a | 14,000,088 | 6,046,182 |
| rmp-serde 1.3.1 | 11.746 ms | 8.4254 ms | 8,125,006 | 6,391,037 |
| ciborium 0.2.2 | 38.030 ms | 76.858 ms | 13,122,324 | 6,759,128 |
| serde_json 1.0.154 | 60.056 ms | 68.771 ms | 26,192,883 | 8,584,671 |

What these show:

1. Even the slowest option (serde_json, about 26 MB in 60 ms, ≈ 0.4 GB/s) is more than 1000× our worst-case write rate (B5). **Serialization speed does not drive the decision.**
2. Raw f32 data compresses poorly with general-purpose codecs (6.0 MB → 5.35 MB with zstd). Column layout plus delta encoding (Parquet `DELTA_BINARY_PACKED`, `BYTE_STREAM_SPLIT`) is what shrinks telemetry. That favours Parquet for derived tables and integer counts with i64-ns timestamps in the raw log.

No published Parquet vs Arrow IPC vs SQLite write benchmark at telemetry-like row widths was found that I could cite. Measure it with a ~50-line harness if it ever matters.

### B4. Browser specifics

- **OPFS `FileSystemSyncAccessHandle`** ([MDN](https://developer.mozilla.org/en-US/docs/Web/API/FileSystemSyncAccessHandle)):
  - Available only in dedicated Workers, secure context only.
  - Methods: `write(buf, {at})`, `flush()`, `truncate()`, `getSize()`. These are now synchronous in all browsers.
  - Takes an exclusive lock on the file.
  - "Baseline widely available since March 2023".
  - `flush()` "persists … to disk", with no stronger durability promise.
  - This is the natural sink for the append-only log.
  - Call `navigator.storage.persist()` to reduce the chance of eviction. Exact eviction behaviour is browser-specific (UNVERIFIED).
- **Official sqlite-wasm persistence** ([sqlite.org](https://sqlite.org/wasm/doc/trunk/persistence.md)):
  - `opfs` needs SharedArrayBuffer and COOP/COEP headers.
  - `opfs-wl` (Web Locks, added in 3.53) needs `Atomics.waitAsync`.
  - **`opfs-sahpool` needs no COOP/COEP**, "works on major browsers released since March 2023", and is the fastest, but allows no concurrent connections to the same DB.
  - All of these are Worker-only.
  - WAL needs `locking_mode=exclusive` and brings no concurrency benefit.
  - Warning: OPFS databases "sometimes disappear for environment-specific reasons", and incognito mode may not persist at all. **Always offer export.**
- **DuckDB-wasm** ([docs](https://duckdb.org/docs/2.0/clients/wasm/overview.html)):
  - "WebAssembly limits the amount of available memory to 4 GB".
  - Single-threaded by default; multithreading is "experimental".
  - Reads Parquet over HTTP with range requests.
  - Good as an **optional in-browser analysis view**, not as storage.
  - The npm package is 149 MB unpacked across all bundles; the per-bundle wasm size is UNVERIFIED.
- **parquet-wasm** ([README](https://github.com/kylebarron/parquet-wasm)) is evidence of what arrow-rs and parquet cost in wasm:
  - "all compression codecs, the brotli-compressed WASM bundle is 1.2 MB".
  - The minimal read-only build without compression is 456 KB brotli.
  - It supports Snappy, Gzip, Brotli, ZSTD and LZ4_RAW in wasm, so zstd-sys does compile to wasm32 in practice.
- **CompressionStream** ([MDN](https://developer.mozilla.org/en-US/docs/Web/API/Compression_Streams_API)) supports gzip and deflate natively and has been Baseline since May 2023. This is a zero-dependency option for compressing exports in the browser. zstd and brotli support is UNVERIFIED.
- **Timer resolution** matters at 8 kHz, where samples arrive every 125 µs. `performance.now()` is coarsened to **100 µs when the page is not cross-origin-isolated and 5 µs when it is** ([MDN](https://developer.mozilla.org/en-US/docs/Web/API/Performance/now)). For meaningful per-event timing the web build needs COOP/COEP. The rate at which pointer events and coalesced events are actually delivered is UNVERIFIED.

### B5. Data-rate arithmetic (from our own spec, not measured)

| Stream | Record | Worst-case rate | Uncompressed |
|---|---|---|---|
| Mouse move | `{t_ns: i64, dx: i32, dy: i32}` = 16 B | 8,000/s during continuous motion | 128 KB/s ≈ **461 MB/h** |
| Frame | ~64 B (t_ns, sim tick, yaw/pitch f64, flags, present timing) | 360/s | 23 KB/s ≈ 83 MB/h |
| Button, spawn, hit and other events | small | rare | negligible |

Aim scenarios usually last minutes, so a full session lands in the tens of MB. Disk and OPFS bandwidth is not a concern. **Browser memory is**: stream chunks to OPFS and never hold the whole session in wasm memory, given the 4 GB wasm32 ceiling and lower practical limits. Turn on lz4 per chunk (lz4_flex) only if sizes become a problem; a flag bit is reserved for it.

---

## C. Recommended storage architecture

```
sessions/<session-uuid>/
  manifest.json            # written at start; rewritten atomically (tmp + rename) at close with completed=true
  raw.kflog                # append-only chunked event log (source of truth for replay)
  derived/                 # produced from raw.kflog at close or by crash recovery; always regenerable
    mouse.parquet  frames.parquet  events.parquet  trials.parquet  session_metrics.parquet
export: <session-uuid>.kfz  (= ZIP, members STORED) {manifest.json, raw.kflog, derived/*.parquet, FORMAT.md, read_kflog.py}
```

1. **Raw log (`.kflog`).** This is the source of truth.
   - **File header** (16 B): magic `b"KFLOG\r\n\x1a"` (PNG-style, so text-mode corruption is detectable), `u32 format_version`, `u32 reserved`.
   - **Chunk:** `u32 payload_len | u8 kind | u8 kind_version | u16 flags (bit0 = lz4) | u32 record_count | payload | u32 crc32(kind..payload)`.
   - **Payload:** a packed array of fixed-size little-endian records for that kind, encoded with `to_le_bytes`. No serde.
   - **Ordering:** chunks of different kinds interleave in time order, and every record carries `t_ns` (i64, monotonic session clock) plus, for simulation events, `tick: u64`. Replay merges by `(t_ns, seq)`.
   - **Writing natively:** flush each chunk with `write` at ≤100 ms or ≤4096 records, and `fdatasync` at ≤1 s and at close. An application crash then loses at most the in-memory chunk; a power loss loses at most about 1 s.
   - **Writing in the browser:** the worker holds a `SyncAccessHandle`, calls `write({at})` for each chunk, and `flush()` at the same cadence.
   - **Recovery:** scan chunks, stop at the first short read or bad CRC, then truncate to that point.
   - **Dependencies:** `crc32fast` only.
   - **Why not Arrow IPC for the raw log:**
     - It takes one schema per stream, so heterogeneous events need several files.
     - The flatbuffer metadata per batch forces large batches, which means more data lost on a crash.
     - Total ordering across kinds becomes a merge problem.
     - Readers in other languages are well served by derived Parquet plus the reference reader instead.
2. **Derived tables: Parquet.** One file per table, written through arrow-rs's `ArrowWriter`.
   - Native builds: zstd.
   - Web builds: snappy or lz4_raw (pure Rust); zstd only if the wasm build has clang.
   - Include `kineforge.format_version`, `scoring_version` and the source `raw.kflog` SHA-256 in the Parquet key-value metadata.
   - Polars, pandas, pyarrow, DuckDB, R arrow, hyparquet and parquet-wasm all read these files.
   - Consider a CSV export option for LoC-style preservation.
3. **Manifest: JSON** with these fields:
   - `schema_version`, `app_version`, `git_sha`, `build_target`, `rustc`
   - `started_at` (UTC wall clock) plus clock source and `t_ns` epoch mapping
   - `rng {algorithm, seeding, seed}`, scenario id and config (inline)
   - input config: DPI, sensitivity, gain formula id, polling Hz, raw-input API
   - display config: refresh Hz, vsync, resolution
   - `files[] {path, bytes, sha256}`
   - `scoring_version`, `completed: bool`, `end_reason`
   - A missing `completed` means the session crashed: recover it and mark it `recovered: true`.
4. **App index.** Scan `sessions/*/manifest.json` plus `session_metrics.parquet` at startup. **Add SQLite only when this measurably slows down.** On native that would be rusqlite with `bundled`, WAL and `synchronous=NORMAL`. On the web it would be sqlite-wasm-rs with sahpool, which is Worker-only and single-connection.
5. **Export bundle: ZIP with stored members.** Python can open it with stdlib `zipfile` alone. The Parquet members are already compressed. Include `FORMAT.md`, the byte-level spec of `.kflog`, and `read_kflog.py`, about 40 lines using `struct` and `zlib.crc32`.

---

## D. Name check: "KineForge" (2026-11-13)

| Registry / place | Result |
|---|---|
| crates.io | `q=kineforge`: **0 results**. Exact `kineforge`, `kineforge-core`, `kineforge-cli`, `kine-forge`, `kine_forge`: all **404 (free)**. Nearby names: `kine`, `kine-core` (unrelated). |
| npm | `kineforge`, `kine-forge`, `@kineforge/core`: **404**. Search total 0. |
| PyPI | `kineforge`, `kine-forge`: **404**. |
| GitHub user/org | `kineforge`, `KineForge`, `kine-forge`: **404 (not taken)**. |
| GitHub repos named kineforge, all 2026 and all 0–1 stars | `znzhao/Kineforge` (AI harness for the game Besiege) · `amgustav/kineForge` (MuJoCo robot policy testbed) · `wieslawsoltes/KineforgeStudio` (MIT, "robotic-cell editor and deterministic six-axis kinematic simulator", GitHub Pages site) · `0dot77/kineforge` (node-based live media for dancers, with brand assets) · `rzfarhad73/kineforge` (SVG animation studio, site kineforge.farhad-rezaei.com) · `Victorius23/KineForge` (Flutter workout app) · `illiaovcharenko1/kineforge-portfolio-` (see below) · `Indoairevo/Kineforge-pro` · `Javy16S/Kineforge-Cloud-Render`. Similar names: `xizon/WebKineForge`, `KinetiForge-Vehicle-System` (UE5). |
| Product Hunt | **"Kineforge AI/ML model for robotics"** launched 2026-06-21 by Brightforge Software Inc. (solo founder), with a Hugging Face Space `Brightforge-Software-Inc/kineforge-demo` and a GitHub Pages site ([PH](https://www.producthunt.com/products/kineforge-ai-ml-model-for-robotics)). This is the most "product-like" use found. |
| Other web | Cinevva Arcade game "KineForge" (browser 3D scene builder, not an aim trainer) ([link](https://app.cinevva.com/arcade/kineforge)). Near-misses: CineForge (UE plugin, and a pending US mark "CINEFORGE" #99382074, Class 9), KineFX (SideFX Houdini, pending US mark #90403667), KINE-FORM (registered 2000, Class 7). |
| Domains | **kineforge.com: registered.** It resolves to OVH (178.32.137.139) and serves "Site not installed – OVHcloud", so there is no live site. kineforge.io/.dev/.ai/.app/.org/.net/.gg: no DNS records. That suggests they are probably unregistered, but WHOIS was not checked. |
| USPTO / EUIPO | **Not reachable programmatically.** tmsearch.uspto.gov is a JavaScript single-page app and its API refused POST. The EUIPO eSearch connection failed (curl error 56). A web search found no "Kineforge" trademark record (UNVERIFIED, since search engines miss filings). Check manually at tmsearch.uspto.gov, euipo.europa.eu/eSearch and WIPO Global Brand Database. |

**Risk: medium.** No package-registry or org collision exists, and nothing was found in gaming, aim training or esports. However, at least nine unrelated 2026 projects use the name, one of them a company-backed product launch in an adjacent "kinematics/motion" space, and the .com domain is held by someone else. If the name is kept, claim the crates.io, npm and PyPI names and the GitHub org early, and choose a non-.com domain. This is not a legal opinion.

---

## Open questions / UNVERIFIED

1. Whether stable Rust 1.100 exposes `f32::mul_add_relaxed`. The intrinsic docs say so, but the stable f32 page I read did not show it. It does not matter as long as it is banned.
2. Whether sqlite-wasm-rs needs a local clang with a wasm32 target. I believe so, because it compiles the SQLite C source, but did not verify.
3. Which browsers implement the Wasm 3.0 deterministic profile. This affects only NaN bits, which we avoid anyway.
4. Achievable pointer-event delivery rate and timestamp fidelity in browsers at 8 kHz. Cross-origin isolation is needed for 5 µs timer resolution.
5. Whether DuckDB-wasm has caught up to DuckDB 2.0. The npm "latest" tag is a dev build from 2026-06.
6. The exact meaning and formula of the "ODD-compatible gain" requirement. If a reference implementation is defined in fixed-point, adopt `fixed` (MSRV 1.93); otherwise use i64 counts × f64 factor.
7. Whether rustfft results differ across its SIMD backends. This only matters if FFT output is ever scored.
8. Status of the trademarks in note D. The registries were unreachable.

## Primary sources (all accessed 2026-11-13)

- crates.io API: `https://crates.io/api/v1/crates/{glam,nalgebra,ultraviolet,mint,parry3d,rapier3d,rand,rand_core,rand_chacha,rand_pcg,rand_xoshiro,statrs,argmin,levenberg-marquardt,biquad,rustfft,idsp,splines,fixed,libm,arrow,arrow-ipc,arrow-array,parquet,ruzstd,zstd,lz4_flex,snap,rusqlite,sqlite-wasm-rs,sqlite-wasm-vfs,serde_json,rmp-serde,flatbuffers,prost,capnp,postcard,bincode,ciborium,crc32fast,crc,polars,duckdb,rkyv,bitcode,zerocopy,brotli,flate2,miniz_oxide,glamx,minicbor,wincode,chacha20,protox,capnpc,flatbuffers-build,zip,sha2,blake3,rubato,sci-rs,enterpolation,kurbo}` plus `/dependencies` for parry3d, rapier3d, argmin, statrs, levenberg-marquardt, idsp, biquad and rustfft.
- Crate tarballs read: glam 0.34.3, libm 0.2.16, bincode 3.0.0 (`https://crates.io/api/v1/crates/<n>/<v>/download`).
- https://rust-lang.github.io/rfcs/3514-float-semantics.html
- https://doc.rust-lang.org/std/primitive.f64.html · https://doc.rust-lang.org/stable/std/primitive.f32.html · https://doc.rust-lang.org/nightly/std/intrinsics/fn.fmuladd.html
- https://webassembly.org/news/2025-09-17-wasm-3.0/ · https://lists.llvm.org/pipermail/llvm-commits/Week-of-Mon-20251013/1753482.html · https://www.rustmax.net/api/core/core_arch/wasm32/relaxed_simd/
- https://rust-random.github.io/book/crate-reprod.html · https://github.com/rust-random/rand/blob/master/CHANGELOG.md
- https://rapier.rs/docs/user_guides/rust/determinism · https://docs.rs/crate/rapier3d/latest/features
- https://github.com/apache/arrow-rs (parquet/Cargo.toml, arrow-ipc/Cargo.toml, .github/workflows/{parquet,arrow}.yml)
- https://arrow.apache.org/docs/format/Columnar.html (spec 1.5, IPC section)
- https://github.com/KillingSpark/zstd-rs (README) · https://github.com/Spxg/sqlite-wasm-rs (README) · https://github.com/rusqlite/rusqlite (README, Cargo.toml)
- https://sqlite.org/wasm/doc/trunk/persistence.md · https://www.sqlite.org/pragma.html#pragma_synchronous
- https://developer.mozilla.org/en-US/docs/Web/API/FileSystemSyncAccessHandle · https://developer.mozilla.org/en-US/docs/Web/API/Compression_Streams_API · https://developer.mozilla.org/en-US/docs/Web/API/Performance/now
- https://duckdb.org/docs/2.0/clients/wasm/overview.html · https://duckdb.org/2025/05/23/arrow-ipc-support-in-duckdb.html · https://github.com/duckdb/duckdb/releases/tag/v2.0.0
- https://github.com/kylebarron/parquet-wasm (README) · npm registry metadata for @duckdb/duckdb-wasm, @sqlite.org/sqlite-wasm, apache-arrow, hyparquet, nodejs-polars
- https://github.com/djkoloski/rust_serialization_benchmark (README, updated 2026-11-05)
- https://github.com/rustsec/advisory-db/blob/main/crates/bincode/RUSTSEC-2025-0141.md
- https://www.loc.gov/preservation/resources/rfs/data.html
- Name check: crates.io, registry.npmjs.org, pypi.org/pypi/<n>/json, `gh api users/<n>`, `gh search repos|code kineforge`, https://www.producthunt.com/products/kineforge-ai-ml-model-for-robotics, https://app.cinevva.com/arcade/kineforge, DNS and HTTP probes of kineforge.{com,io,dev,ai,app,org,net,gg}
