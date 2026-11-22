# Vendored C reference engine

`directional_dpi.c`, `directional_dpi.h` and `LICENSE` are verbatim copies from
Open Directional DPI:

- Origin: https://github.com/0zLloen/open-directional-dpi
- Commit: `c177d23c2d6243f676ab49a9531ae120fd0d896c` (`core/` unchanged up to `3ea27cf`)
- Paths: `core/directional_dpi.c`, `core/directional_dpi.h`, `LICENSE`
- Licence: MIT (see `LICENSE`)

| File | SHA-256 |
|---|---|
| `directional_dpi.c` | `18656353ea0c97747387e09a68833764da284ee1065f1e5b419d32a2a83bfba1` |
| `directional_dpi.h` | `b63febea2c2e5a00b36f3ccfc35089c5ac303e5fded225c89aa7f1415e0d1284` |
| `LICENSE` | `0b99bfe4191c3f5e2774f0f498ea9b40208d90707064359059ac868a4180eb1c` |

Do not edit these files; re-vendor them from upstream
(`git show <commit>:core/directional_dpi.c > directional_dpi.c`, likewise for the
header and licence), update the commit and hashes above, then regenerate
`crates/kf-dirgain/tests/vectors/traces.json` (see `../README.md`).

`kf_shim.c` is KineForge's own: layout facts for the Rust `#[repr(C)]` mirrors and
linkable copies of the header's `static inline` helpers.
