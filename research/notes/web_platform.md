# KineForge research: browser runtime, GPU, storage and delivery (web build)

Scope: the browser as a first-class target (Rust → `wasm32-unknown-unknown`, static hosting, no installer or account, local-first data).

Not covered here, because other notes already do:
- Input semantics (Pointer Lock, `unadjustedMovement`, rAF-aligned input, `pointerrawupdate`, per-browser `event.timeStamp` and timer coarsening from source): `_browser.md`.
- wgpu backends, present modes, egui/AccessKit and crate versions: `rust_render_ui.md`.
- OPFS sync-handle telemetry log, sqlite-wasm, Parquet: `rust_math_data.md`.

This note cross-references those notes and does not repeat them.

Access dates: **[13]** = 2026-11-13, **[14]** = 2026-11-14. **UNVERIFIED** = not confirmed from a primary source or a local run.

---

## 0. Decisive findings

1. **WebGL2 fallback is mandatory, and Linux is where it bites.**
   - **Firefox stable on Linux has no WebGPU.** BCD says "Does not support Linux. See bug 2006676". Local Firefox 159 shows `'gpu' in navigator === false`.
   - **Firefox on Intel Macs has no WebGPU either** (bug 2004105).
   - **Chrome on Linux** ships WebGPU only for Intel Gen12+ (since 144) and NVIDIA ≥535.183.01 **on Wayland** (since 147).
   - **Local check:** headless Chromium 156 on this X11 + NVIDIA box exposes `navigator.gpu`, but `requestAdapter()` returns `null`. So detect support by trying to get an adapter, never by checking that `navigator.gpu` exists.
   - **Survey data:** web3dsurvey puts working-adapter share at **Linux 19.4%** and **Firefox 62.1%**, against 81.9% overall.
2. **Cross-origin isolation (COOP `same-origin` + COEP `require-corp`) is worth serving from day 1**, even without threads.
   - It is the only route to 5 µs `performance.now()` in Chrome and 20 µs in Firefox. Without it the resolution is 100 µs in Chrome and **1 ms in Firefox**, both measured locally.
   - It also unlocks SharedArrayBuffer and threads later.
   - It is free for a fully same-origin app. **GitHub Pages cannot set headers.** Cloudflare Pages/Workers and Netlify can, via `_headers`.
3. **WebGPU GPU timestamps do not benefit from COI.**
   - The spec coarsens them with `crossOriginIsolatedCapability=false`, which means 100 µs.
   - Chrome documents 100 µs quantization, which is lifted only by a developer flag.
   - WebGL2 via wgpu has **no** GPU timestamps at all.
   - So GPU-side frame timing on the web is coarse. Plan CPU-side `performance.now()` around submit and rAF instead.
4. **The web build cannot be faster than the display, and on Safari it cannot even reach it.**
   - There are no present modes. Frames are presented in the event loop's "update the rendering" step. `getContext()` options, including `desynchronized`, are ignored for `'webgpu'`.
   - Chrome and Firefox run rAF at the display rate (240–540 Hz reported).
   - **Safari defaults to ~60 Hz rAF** behind the "Prefer Page Rendering Updates near 60fps" flag, drops to ~30 fps in Low Power Mode, and runs iframes at 30 fps.
   - Hidden tabs get **no rAF**, and their timers are throttled to 1 s or 1 min.
5. **Threads are still nightly-only**, so ship single-threaded on stable.
   - They need nightly, `-Zbuild-std`, `+atomics`, and since Oct 2025 also explicit `--shared-memory --import-memory --max-memory` and TLS export linker args.
   - Single-threaded is enough for an aim trainer. Use a plain JS worker plus `postMessage` with transferable buffers for OPFS writes.
6. **Storage is durable enough only with `persist()` and an export path.**
   - Quotas: Chrome allows ~60% of disk per origin; Firefox best-effort allows min(10% of disk, 10 GiB); Safari allows ~60% per origin.
   - **Safari deletes all script-writable storage after 7 days of Safari use without interaction** unless the site is a Home Screen/Dock app.
   - Firefox prompts for `persist()`; Chrome and Safari decide by heuristic.
   - `showSaveFilePicker` is Chromium-only, so fall back to a Blob download.
7. **Bundle size is dominated by wgpu-core + naga when WebGL2 is compiled in.**
   - Measured on deployed builds (§3.6): the wgpu examples' WebGPU build has 2.23 MB of code; the WebGL2-capable build has 8.33 MB.
   - The egui.rs demo is 11.5 MB raw and 4.5 MB gzip.
   - GitHub Pages serves gzip only (observed).
8. **Architecture: (A) one shared Rust codebase including wgpu rendering**, with a hard seam that keeps the sim, input math and scoring core free of wgpu, winit and web-sys (B's seam without B's duplication).
   - Web a11y is the main gap. egui on the web has only an experimental speech-synthesis "screen reader", and the AccessKit web adapter is "planned".
   - Add DOM menus (C) later only if a11y or SEO demands them.

---

## 1. WebGPU availability, features, limits, canvas

### 1.1 Availability matrix (stable channels)

| Browser | Windows | macOS | Linux | ChromeOS | Android | iOS/iPadOS |
|---|---|---|---|---|---|---|
| Chrome / Edge / Opera (Dawn) | ✅ 113 (x86/x64); **ARM64: flag only** | ✅ 113 | ✅ **Intel Gen12+ since 144**; **NVIDIA driver ≥535.183.01 on Wayland since 147**; others need flags | ✅ 113 | ✅ 121 (Android 12+, ARM/Qualcomm/Intel); Imagination Android 16+ since 139; Samsung Xclipse and others TBD | n/a (WebKit) |
| Firefox (wgpu) | ✅ 141 | ✅ Apple Silicon: 145 on macOS 26, 147 on all macOS; **Intel Macs: no** (bug 2004105) | ❌ Nightly only; "Mozilla expects to ship on Linux in 2026"; not in 159 (2026-10-27) | — | ❌ flag (`gfx.webgpu.ignore-blocklist`, Beta/Nightly) | n/a |
| Safari (WebKit) | — | ✅ 26 (caniuse marks Safari 26.0–27.x "partial" with no note) | — | — | — | ✅ 26 (also visionOS 26) |
| Samsung Internet | — | — | — | — | ✅ 24+ (caniuse) | — |

Sources:
- gpuweb Implementation-Status wiki (last edited by Kai Ninomiya, 2026-10-02) [13]: https://github.com/gpuweb/gpuweb/wiki/Implementation-Status
- MDN BCD `api/GPU.json` @ commit 69fcf36081 (2026-11-13). Firefox notes cite bugs 1942431 (no service workers), 1972486, 1992212, 1993341, 2004105 (no Intel Mac) and 2006676 (no Linux) [13]: https://github.com/mdn/browser-compat-data/blob/main/api/GPU.json
- caniuse [13]: https://caniuse.com/webgpu
- Chrome "What's New in WebGPU 147–148" (2026-04-22): "support is expanding to include modern NVIDIA drivers (2024-05) on Wayland" [13]: https://developer.chrome.com/blog/new-in-webgpu-147-148
- Chrome 155–156 post: `shader-f16` "now supported on Linux for recent NVIDIA GPU drivers"; srgb-linear/display-p3-linear canvas colour spaces; "texture compression unaligned" [13]: https://developer.chrome.com/blog/new-in-webgpu-155-156
- Firefox 159 release notes (2026-10-27): no WebGPU item [13]: https://www.firefox.com/en-US/firefox/159.0/releasenotes/
- Field data: web3dsurvey (no collection window stated) [14]: https://web3dsurvey.com/webgpu
  - Working adapter: 81.86% overall.
  - By OS: ChromeOS 93.3%, macOS 91.0%, iOS 88%, Windows 84.4%, Android 76.6%, **Linux 19.4%**.
  - By browser: **Firefox 62.1%**.
  - Fallback adapters: 0.04%.

Linux workaround flags (wiki): `--enable-unsafe-webgpu --ozone-platform=x11 --use-angle=vulkan --enable-features=Vulkan,VulkanFromANGLE`. These are fine for us and our CI, never for users.

### 1.2 Spec facts (WebGPU Editor's Draft 2026-11-10; W3C TR is a CR Draft dated 2026-09-15) [13]
Sources: https://gpuweb.github.io/gpuweb/ and https://www.w3.org/TR/webgpu/

- **Secure context only.** Every interface is `[Exposed=(Window, Worker), SecureContext]`, `WorkerNavigator includes NavigatorGPU`, and `navigator.gpu` is `[SameObject, SecureContext]`. `http://localhost` and `127.0.0.1` count as secure: the local probe reported `isSecureContext=true` on `http://127.0.0.1`.
- **Default limits (core / compatibility):**

  | Limit | Core | Compatibility |
  |---|---|---|
  | `maxTextureDimension2D` | 8192 | 4096 |
  | `maxBufferSize` | 256 MiB | — |
  | `maxStorageBufferBindingSize` | 128 MiB | — |
  | `maxComputeWorkgroupStorageSize` | 16 KiB | — |
  | `maxStorageBuffersPerShaderStage` | 8 | — |
  | `maxStorageBuffersInVertexStage` | 8 | **0** |
  | `maxColorAttachments` | 8 | 4 |

  Compatibility mode is in the spec as `requestAdapter({featureLevel:"compatibility"})`. Chrome supports it on OpenGL ES 3.1 since 146.
- **Real-world limits (web3dsurvey [14]):**
  - `maxTextureDimension2D` ≥16384 on 99%.
  - `maxStorageBuffersPerShaderStage` ≥10 on 98%, ≥16 on 80%.
  - `maxBufferSize` ≥1.25 GB on 86%.
  - Features: `timestamp-query` 98.8%, `shader-f16` 94.1%, `float32-filterable` 91.3%.
  - There is no per-browser breakdown, so per-browser feature and limit tables for Firefox and Safari are **UNVERIFIED**.
- **Timestamp queries.** The spec gets the "current queue timestamp" by calling *coarsen time* "with crossOriginIsolatedCapability set to false". Its note says: "Cross-origin isolation never applies to the device timeline or queue timeline."
  - Per HR-Time, that means **100 µs** (https://w3c.github.io/hr-time/, ED 2026-11-06 [13]).
  - Chrome: "timestamp queries are quantized with a resolution of 100 microseconds". The quantization is disabled only via `chrome://flags/#enable-webgpu-developer-features` (https://developer.chrome.com/docs/web-platform/webgpu/developer-features, https://developer.chrome.com/blog/new-in-webgpu-120 [13]).
  - The spec also warns that values are implementation-defined and may go backwards, so "these data points can safely be discarded".
- **Device loss.**
  - `GPUDevice.lost` resolves with `GPUDeviceLostInfo{reason: "unknown"|"destroyed", message}`. Device loss itself generates no errors.
  - After loss, validation and error scopes stop reporting. Most operations "appear to succeed", but `mapAsync()` errors.
  - "An invalid object will never become valid at a later time."
  - An adapter becomes **"consumed"** after a successful `requestDevice()`, and adapters "may also expire at any time".
  - **Recovery = `requestAdapter()` again → `requestDevice()` → recreate every GPU resource** (pipelines, buffers, textures, and canvas `configure()`).
  - On loss, a canvas *may* keep showing `[[lastPresentedImage]]` (an optional fallback) instead of going blank.
  - KineForge policy: abort the current trial, write a `device_lost` event to telemetry, rebuild the renderer from CPU-side state, and continue with the next trial.
  - The wgpu-level API name for the lost callback in v30 was not checked (**UNVERIFIED**).
- **Canvas presentation (§21).**
  - There are no present modes and no frame-latency knob.
  - "Any changes to the drawing buffer made through the currentTexture get presented when updating the rendering of a WebGPU canvas." This happens in "update the rendering" of a Document **or a dedicated worker**.
  - `getCurrentTexture()` returns the same (destroyed) texture for the rest of the frame.
  - A **standalone `new OffscreenCanvas()` is never presented**. Only canvases from `transferControlToOffscreen()` are.
  - The context-creation options dictionary "is ignored", so `desynchronized`/low-latency hints do not exist for WebGPU.
  - Per BCD, `desynchronized` exists only for 2D/WebGL contexts: Chrome 81 (ChromeOS/Windows), Chrome Android 75, Safari 15 (2D only), not Firefox.
- **`GPUCanvasConfiguration`.**
  - Fields: `device`, `format` ∈ {`bgra8unorm`, `rgba8unorm`, `rgba16float`} (sRGB only via `viewFormats` plus `createView`), `usage` (default `RENDER_ATTACHMENT`), `viewFormats`, `colorSpace` (default `"srgb"`), `toneMapping{mode: "standard"|"extended"}`, `alphaMode` (`"opaque"` default | `"premultiplied"`).
  - `"opaque"` "incurs a clear of the alpha channel" unless the implementation can ignore alpha.
  - BCD notes: `toneMapping` is Chrome 129+ only (Firefox and Safari: no). The **`rgba8unorm` canvas format is not supported on macOS Chrome** (crbug 40823053), so always use `getPreferredCanvasFormat()`.
- **Workers and OffscreenCanvas.**
  - Per BCD, `OffscreenCanvas.getContext('webgpu')` is available in Chrome 113 (Linux limits as above), Firefox 141 (same platform limits; **not in service workers**) and Safari 26.
  - Local run: in Chromium 156 a dedicated worker gets a WebGPU OffscreenCanvas context.
  - **Pitfall (local, Firefox 159 Linux):** `new OffscreenCanvas(8,8).getContext('webgpu')` returns a `GPUCanvasContext` even though `navigator.gpu` is absent. Feature-detect only via `navigator.gpu?.requestAdapter()`.

### 1.3 Should we render from a worker? (OffscreenCanvas)
- Pros: a busy main thread, such as DOM UI or GC, no longer stalls rendering.
- Cons:
  - Input (Pointer Lock `pointermove`/`pointerrawupdate`) is dispatched **only on the main thread**, so every input sample costs a `postMessage` hop. Avoiding that hop needs a SharedArrayBuffer ring buffer, which needs COI.
  - The worker has its own rAF and its own "update the rendering".
  - winit's web backend assumes the main thread (**UNVERIFIED** in the 0.30 docs).
- **Recommendation for v1: everything on the main thread.** The in-game UI is canvas-only, so main-thread contention is low. Revisit only if profiling shows main-thread jank.

---

## 2. WebGL2 fallback through wgpu

- **Availability.** WebGL2 is universal in current engines. Locally, Chromium 156 used ANGLE on GL (NVIDIA) with MAX_TEXTURE_SIZE 32768 and exposes `EXT_disjoint_timer_query_webgl2`. Firefox 159 reports a sanitized renderer string ("NVIDIA GeForce GTX 980, or similar"), MAX_TEXTURE_SIZE 32768, and **no** timer-query extension.
- **One wasm binary, runtime choice.**
  - Since wgpu v0.19: "Enabling `webgl` no longer removes the `webgpu` backend … If both `webgl` & `webgpu` are enabled, `wgpu::Instance` decides upon creation whether to target wgpu-core/WebGL or WebGPU." (CHANGELOG, PR #5044.)
  - The helper is `wgpu::util::new_instance_with_webgpu_detection`, together with `is_browser_webgpu_supported` (PR #6371). Its source (`wgpu/src/util/init.rs` [13]) checks `navigator.gpu` **and** awaits one `requestAdapter()`, "since as of writing Chrome on Linux has the `gpu` property but doesn't support WebGPU". It returns false outside the main thread or a dedicated worker.
  - `webgpu` is a default feature of wgpu 30. `webgl` is opt-in (`wgpu/Cargo.toml` [13]).
- **Local caveat (headless Chromium 156 with `--enable-unsafe-webgpu`).** In 3 of 3 runs the **first** `requestAdapter()` in the page returned `null` and every later call succeeded:
  - `{}` gave nvidia/ampere.
  - `low-power` gave amd/gcn-5.
  - `forceFallbackAdapter` gave google/swiftshader.

  A single-shot detection would therefore fall back to WebGL2. This was only observed in flagged and headless configurations. If we see it in the field, retry once before falling back (**UNVERIFIED** for headed and default configurations).
- **WebGL2 path limits** (wgpu-types `Limits::downlevel_webgl2_defaults()` and `wgpu-hal/src/gles/adapter.rs` [13]):
  - **Zero storage buffers and storage textures in every stage.** `max_compute_*` = 0, so no compute shaders (GLES 3.0 lacks SSBO and compute; those need ES 3.1).
  - `max_uniform_buffers_per_shader_stage` 11.
  - `max_inter_stage_shader_variables` 15.
  - `max_vertex_buffer_array_stride` 255.
  - The 2D texture default limit is 2048 (`downlevel_defaults`, conservative). The real adapter limit is usually higher, so request `adapter.limits()`.
  - Flags off on WebGL: `BASE_VERTEX` (needs 3.2), `INDEPENDENT_BLEND` (3.2 or `EXT_draw_buffers_indexed`), `CUBE_ARRAY_TEXTURES`, `LINEAR_INTERPOLATION` (GLSL ES has no `noperspective`), `UNRESTRICTED_INDEX_BUFFER`, `BUFFER_BINDINGS_NOT_16_BYTE_ALIGNED`, `UNRESTRICTED_EXTERNAL_TEXTURE_COPIES`.
  - **`TIMESTAMP_QUERY` is enabled only with `GL_ARB_timer_query`, which is desktop GL**, so the WebGL extension is not used.
- **Shader path.** WGSL is translated by naga to GLSL ES 3.00 ("uses Naga to translate WGSL programs into GLSL", README). Without `webgl`, WGSL is passed straight to the browser and no naga is linked.
- **Design rule.** Write all shaders against the WebGL2 subset: vertex and uniform buffers only, no storage buffers or compute. Instancing via vertex buffers covers targets and particles.
- **Size cost.** See §3.6 and the egui +2.4 MiB figure in `rust_render_ui.md`.

---

## 3. Rust → wasm toolchain

### 3.1 Project status

| Item | Status | Source |
|---|---|---|
| rustwasm GitHub org | WG archived 2024. "Sunsetting" post 2025-07-21: org archived Sept 2025. **wasm-bindgen moved to the new `wasm-bindgen` org** with new maintainers. Other repos were archived or handed to their maintainers. | https://blog.rust-lang.org/inside-rust/2025/07/21/sunsetting-the-rustwasm-github-org/ [13] |
| wasm-bindgen | 0.2.130 (2026-10-30). Repo `wasm-bindgen/wasm-bindgen`, pushed 2026-11-13. | crates.io API, GitHub API [13] |
| wasm-pack | **Transferred, alive.** `wasm-bindgen/wasm-pack` 0.15.1 (2026-10-30). `rustwasm/wasm-pack` → 301. | same |
| gloo | → `ranile/gloo` (active). twiggy → `AlexEne/twiggy` (archived). | GitHub API [13] |
| console_error_panic_hook | 0.1.7 (2021). Repo `rustwasm/console_error_panic_hook` **archived**. It still works, but it is a ~10-line `std::panic::set_hook` + `web_sys::console::error_1`, so inline it. | crates.io, GitHub API [13] |
| Trunk | 0.22.1 (2026-10-26), `trunk-rs/trunk`. `data-wasm-opt` (0/1/2/3/4/s/z, release only). **`data-bindgen-target` default `no-modules`.** `data-type="worker"` with `data-loader-shim`. `data-keep-debug` (use with `data-wasm-opt="0"`). `data-integrity` default sha384. | https://trunk-rs.github.io/trunk/guide/assets/ [13] |
| binaryen (wasm-opt) | version_133 (2026-09-21) | GitHub releases API [13] |
| wasm-bindgen-rayon | 1.3.0 (2024-12-21), moved to `RReverser/wasm-bindgen-rayon`. The GoogleChromeLabs repo is archived. | GitHub API [13] |
| Rust stable | 1.100.0 (2026-11-12) | GitHub releases API [13] |
| cargo-leptos | Irrelevant: it is a Leptos SSR/fullstack build tool, and we have no server. | — |

- **CLI version pinning.**
  - The wasm-bindgen CLI and the `wasm-bindgen` crate must match exactly. A mismatch fails with a "schema version" error (community threads, e.g. https://users.rust-lang.org/t/cant-compile-project-for-wasm-some-weird-version-mismatch/79874 [13]).
  - wgpu's own CI does it this way: `cargo metadata … | jq '.packages[]|select(.name=="wasm-bindgen")|.version'`, then `cargo install wasm-bindgen-cli --version=$V` (https://github.com/gfx-rs/wgpu/blob/trunk/.github/workflows/ci.yml, job `wasm-test` [13]).
  - wasm-pack reads `Cargo.lock` to choose the version (docs.rs `wasm_pack/lockfile.rs`).
  - Whether Trunk auto-matches the `Cargo.lock` version is **UNVERIFIED**.
- **Recommended build (ponytail).** `cargo build --release --target wasm32-unknown-unknown` → `wasm-bindgen --target web` (version-pinned as above) → `wasm-opt -Oz` (optional, measure) → static `index.html` with `<script type="module">import init from './kf.js'; await init();</script>`.
  - Trunk is fine as a convenience, but remember to set `data-bindgen-target="web"`.
  - Vite is only needed if a TS/DOM UI appears (strategy C or D).

### 3.2 Target features and the panic strategy
Source: https://doc.rust-lang.org/nightly/rustc/platform-support/wasm32-unknown-unknown.html [13]

- `wasm32-unknown-unknown` (Tier 2) enables LLVM `generic` defaults: **multivalue, mutable-globals, reference-types, sign-ext, plus nontrapping-fptoint and bulk-memory since Rust 1.87 / LLVM 20.**
- Engine support for all of these predates WebGPU (BCD `webassembly/*`): multi-value Chrome 86/Firefox 78/Safari 13.1; reference-types 96/79/15.1; bulk-memory 75/79/15.1. So there is no compatibility concern for any WebGPU-capable browser.
- To go back to MVP, use `wasm32v1-none` (no_std, Core 1.0 + mutable-globals) or nightly `-Ctarget-cpu=mvp -Zbuild-std`. Not needed.
- `simd128` is opt-in (`-Ctarget-feature=+simd128`). It validates in Chromium 156 and Firefox 159 (local) and in Safari 16.4+ (BCD).
- **Panic strategy.** `panic=abort` is the default. `panic=unwind` needs nightly + `-Zbuild-std` + `-Cllvm-args=-wasm-use-legacy-eh=false`. Final exception handling (exnref) is in Chrome 137, Firefox 131 and Safari 18.4 (BCD). Stay on abort.
- **Size profile.** Example numbers from Bevy's examples README (small 3D scene, July 2022, uncompressed):

  | Profile | Size |
  |---|---|
  | default | 13.0 MB → **8.5 MB** after `wasm-opt` |
  | `opt-level="z"` + fat LTO + `codegen-units=1` | 8.5 MB → **4.8 MB** after `wasm-opt -Oz` |

  Source: https://github.com/bevyengine/bevy/blob/main/examples/README.md#optimizing [13]

  For KineForge, keep `opt-level=3` for the sim and measure "s"/"z", because size and speed trade off.
- **Delivery.**
  - `WebAssembly.instantiateStreaming` "rejects with a TypeError" if the MIME type isn't `application/wasm` (https://developer.mozilla.org/en-US/docs/WebAssembly/Reference/JavaScript_interface/instantiateStreaming_static, modified 2026-09-22 [14]).
  - All three static hosts tested serve `application/wasm`; GitHub Pages was observed directly (§10).
- **CSP.** `script-src 'wasm-unsafe-eval'` (Chrome 97, Firefox 102, Safari 16; BCD) allows wasm compilation without JS `eval`.
- **Debugging.**
  - Chrome DevTools debugs DWARF via the "C/C++ DevTools Support (DWARF)" extension and has a Memory Inspector (https://developer.chrome.com/docs/devtools/wasm [14]). The page only documents Emscripten C/C++; **Rust stepping and variable support is UNVERIFIED**.
  - Keep DWARF with `wasm-bindgen --keep-debug` or `--split-debug-info` (writes `<name>_bg.debug.wasm`) (https://wasm-bindgen.github.io/wasm-bindgen/reference/cli.html [13]).
  - Ship release builds stripped. The wgpu examples ship a 1.8–5 MB `name` section (§3.6), which is avoidable bloat.

### 3.3 wasm32 memory and memory64
- **wasm32.** Chrome allows up to 4 GB "up from the previous limit of 2GB" (V8 blog 2020-05-14, testing on M83; https://v8.dev/blog/4gb-wasm-memory [14]). Firefox, Safari and mobile per-tab ceilings are **UNVERIFIED**; iOS is reported to be much lower, also UNVERIFIED.
- **Our need.** Tens of MB per session (`rust_math_data.md` §B5), streamed to OPFS. Reserve the capacity of telemetry buffers at startup so `memory.grow` happens rarely, because growth detaches existing JS `ArrayBuffer` views; wasm-bindgen re-creates its views. The cost of a grow mid-trial is **UNVERIFIED**, so measure it.
- **memory64** (BCD): Chrome 133, Firefox 134, Safari **27.3**. caniuse places 27.3 at the TP boundary, so treat Safari as not yet shipped (UNVERIFIED).
  - It validates locally in Chromium 156 and Firefox 159.
  - Penalty: bounds checks cost "10% to over 100%" (SpiderMonkey blog 2025-01-15, https://spidermonkey.dev/blog/2025/01/15/is-memory64-actually-worth-using.html [13]).
  - The Rust target `wasm64-unknown-unknown` is **Tier 3** with no prebuilt std (https://doc.rust-lang.org/nightly/rustc/platform-support/wasm64-unknown-unknown.html [13]). wasm-bindgen support is **UNVERIFIED** (PR #5004 seen only as a title).
  - **Don't use it.**

### 3.4 Threads
- **Browser side.**
  - SharedArrayBuffer needs `crossOriginIsolated`, which needs COOP `same-origin` plus COEP `require-corp` or `credentialless`.
  - COEP `credentialless` is in Chrome 96 and Firefox 119; **Safari does not support it** (BCD). Use `require-corp`, which costs nothing for a same-origin app.
  - Wasm threads/atomics are in Chrome 74, Firefox 79 and Safari 15.2 (BCD).
- **Rust side (still nightly as of 2026-11).**
  - Needs `+atomics,+bulk-memory` and `-Zbuild-std=panic_abort,std`.
  - Since rust#147225 (merged 2025-10-02, "Don't enable shared memory by default with Wasm atomics") it also needs `-Clink-args=--shared-memory,--max-memory=<N>,--import-memory,--export=__wasm_init_tls,--export=__tls_size,--export=__tls_align,--export=__tls_base`.
  - Since rust#156174 (May 2026) it also needs `--export=__heap_base`.
  - Sources: https://github.com/rust-lang/rust/pull/147225 [13]; wasm-bindgen raytrace example (nightly + build-std, https://wasm-bindgen.github.io/wasm-bindgen/examples/raytrace.html [13]); wasm-bindgen-rayon README (tested `nightly-2025-11-15`, needs `--target web`, https://github.com/RReverser/wasm-bindgen-rayon [13]).
  - A third-party 2026 write-up warns that `wasm-opt` can silently drop shared memory unless it is given `--enable-threads`: https://garrett-build.pages.dev/posts/silent-regressions-wasm-threads [13] (secondary).
- The main thread cannot block with `Atomics.wait`, so thread pools must be driven from workers (general platform rule).
- **Fallback strategy (the v1 plan).**
  - Single-threaded wasm on stable Rust.
  - Off-main-thread work goes through a **plain dedicated worker** with `postMessage` and transferable `ArrayBuffer`s, which needs no COI. The OPFS writer is the case in point (sync access handles exist only in dedicated workers).
  - Analysis runs post-session or natively.

### 3.5 Main-thread jank sources
- **JS GC.** Avoid per-frame allocations crossing the boundary: `String`, `Closure::new`, `JsValue` arrays. Reuse typed views.
- **`memory.grow`.** See §3.3.
- **Shader compilation on the first use of a pipeline.** Create all pipelines at load time.
- **`getCurrentTexture` stalls when the GPU is behind.** These are general engineering points (**UNVERIFIED** magnitudes); measure with the Chrome Performance panel.

### 3.6 Published and measured wasm sizes
Downloaded [13]. Sections were parsed with a small LEB128 reader.

| Artifact (deployed) | Raw | gzip -9 | Served | Sections |
|---|---|---|---|---|
| egui.rs demo `egui_demo_app_bg.wasm` (GitHub Pages, last-modified 2026-11-13) | 11.5 MB | 4,523,837 B | gzip, `content-length: 4549922`, `application/wasm`, `cache-control: max-age=600` | code 9.57 MB, data 2.39 MB, **stripped** |
| wgpu.rs examples, `webgpu_bg.wasm` (all ~30 examples + assets) | 8,104,615 B | 2,820,455 B | `application/wasm` | **code 2.23 MB**, data 4.03 MB, name 1.79 MB |
| wgpu.rs examples, `webgl2_bg.wasm` (same examples, `webgl` feature ⇒ wgpu-core + hal + naga) | 18,017,201 B | 5,410,536 B | same | **code 8.33 MB**, data 4.55 MB, name 5.04 MB |

- The wgpu example builds' profile is unknown and they are not stripped, so treat them as an upper bound. The +6.1 MB code delta is larger than egui's measured +2.4 MiB (`rust_render_ui.md`).
- **Our own measurement is the deliverable.** The budget target is 2–4 MB gzip for webgpu+webgl+egui (an estimate).

---

## 4. Static hosting and COOP/COEP

| Host | Custom headers (COOP/COEP/CSP/Cache-Control) | Notes |
|---|---|---|
| **GitHub Pages** | **No.** Long-standing request: https://github.com/orgs/community/discussions/13309 [13] | Observed on egui.rs [13]: gzip (no brotli), `cache-control: max-age=600` for everything, correct `application/wasm`. The COI workaround is **coi-serviceworker** (below). |
| **Cloudflare Pages** | **Yes, `_headers`**: ≤100 rules, ≤2000 chars per line. Headers are **not applied to Pages Functions responses**. | https://developers.cloudflare.com/pages/configuration/headers/ (updated 2026-11-12) [13]. Cloudflare steers new projects to **Workers static assets**, which also read `_headers` but are listed as "partially supported" in its compatibility matrix (https://developers.cloudflare.com/workers/static-assets/compatibility-matrix, **UNVERIFIED detail**). |
| **Netlify** | **Yes**: `_headers` or `netlify.toml [[headers]]`. Applies to files "Netlify serves from our own backing store", not to Functions or proxies. | https://docs.netlify.com/manage/routing/headers/ (updated 2025-12-01) [13]. COOP/COEP are not on its ignored list. |

**coi-serviceworker** (https://github.com/gzuidhof/coi-serviceworker, README [13]; repo last pushed 2023-12):
- On first load it registers a service worker and **reloads the page**.
- It must be a separate file on our own origin (not a CDN), served over HTTPS or localhost.
- `coepCredentialless` with a `coepDegrade` fallback.
- Other caveats:
  - Reported reload loops (https://dev.to/stefnotch/enabling-coop-coep-without-touching-the-server-2d3n).
  - **It conflicts with our own PWA service worker**: one SW per scope, so the header injection has to be merged into our SW.
  - A hard reload (Shift+Reload) bypasses the SW and loses COI for that load (**UNVERIFIED**, standard SW behaviour).
- ⇒ **Prefer Cloudflare or Netlify for the canonical deploy**, and keep GitHub Pages as a mirror with merged SW-based COI.

Iframe embedding (itch.io-style portals):
- A cross-origin iframe is COI only if the parent is too.
- Safari caps iframes at 30 fps (WebKit bug 173434, comment 38).
- ⇒ **Ship as a top-level page.**

Suggested `_headers` (assumes Trunk's inline loader is replaced by an external module file, or hashed: **verify CSP against the actual build output**):
```
/*
  Cross-Origin-Opener-Policy: same-origin
  Cross-Origin-Embedder-Policy: require-corp
  Content-Security-Policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; connect-src 'self'; img-src 'self' data: blob:; style-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'
  X-Content-Type-Options: nosniff
/assets/*
  Cache-Control: public, max-age=31536000, immutable
```
`connect-src 'self'` makes "no telemetry upload by default" a browser-enforced guarantee, not just a promise.

---

## 5. Timing

- **`performance.now()` resolution.** Spec: 100 µs non-COI, 5 µs COI (HR-Time ED 2026-11-06; MDN [13]). Per-browser source details, including Safari's fixed 1 ms, are in `_browser.md` §6.6.

  **Local measurements** (headless; 200 ms busy loop; smallest observed step):

  | Browser | Not isolated | COOP+COEP |
  |---|---|---|
  | Chromium 156 | **0.1 ms** | **0.005 ms** |
  | Firefox 159 | **1 ms** (single step value) | **0.02 ms** |

- **rAF rate.**
  - **Chrome and Firefox** follow display vsync with no fixed cap. WebKit bug 173434 reports 240 fps on 240 Hz, 480 fps on 480 Hz, and 540 fps on 540 Hz in Chrome/Firefox (bug report text, a secondary measurement) [13].
  - **Safari** keeps "Prefer Page Rendering Updates near 60fps" **on by default on macOS, iOS and iPadOS**. Turning it off gives 120 Hz on ProMotion. Low Power Mode gives ~30 fps. Iframes get 30 fps.
  - The bug is still NEW; last modified 2026-05-19. A Nov 2025 comment reports a 144 Hz external display giving 120 fps with the flag off: https://bugs.webkit.org/show_bug.cgi?id=173434 [13].
  - Whether Safari 26/27 changed the default is **UNVERIFIED**.
  - Mixed-refresh multi-monitor behaviour is **UNVERIFIED**; secondary sources say Chrome follows the monitor holding most of the window.
  - Headless runs report ~60 Hz synthetic frames, so they are meaningless for this question.
  - KineForge should **measure and display the achieved rAF rate**, and warn on Safari.
- **Background.**
  - rAF "will wait for the page to be visible" (Chrome).
  - Timers in hidden tabs: Chrome checks them 1/s, and after 5 min hidden, silent ≥30 s, chain ≥5 and no WebRTC, 1/min ("intensive throttling", Chrome 88; https://developer.chrome.com/blog/timer-throttling-in-chrome-88 [13]).
  - Firefox desktop: 1 s minimum. Firefox Android: 15 min, and it may unload the tab. An `AudioContext` exempts the tab.
  - A 4 ms clamp applies after 5 nested timeouts (MDN setTimeout, modified 2026-10-26, https://developer.mozilla.org/en-US/docs/Web/API/Window/setTimeout [14]).
  - ⇒ On `visibilitychange`→hidden, or on pointer-lock loss: **pause the trial and mark the telemetry gap**. Never sample with timers.
- **`scheduler.postTask` / `scheduler.yield`.** Chrome 94/129, Firefox 142/142, **Safari: none** (BCD). It is not needed for the game loop.
- **Input vs rAF.** Chrome dispatches continuous events (`pointermove`/`mousemove`/`wheel`) "right before the requestAnimationFrame() callback"; discrete events (down/up/key) dispatch immediately (https://developer.chrome.com/blog/aligning-input-events [13]). `pointerrawupdate` bypasses this alignment: Chrome 77, Firefox 148 (movement fixed), Safari none, secure context only. Details and source lines are in `_browser.md` §6.5.

---

## 6. Storage

| | Chrome/Edge | Firefox | Safari 17+ |
|---|---|---|---|
| Per-origin quota | ~**60% of disk**, best-effort and persistent alike. The browser uses ≤80% of disk overall before evicting best-effort origins. | Best-effort: **min(10% of disk, 10 GiB group limit)**. Persistent: 50% of disk, cap 8 TiB. | Browser app: ~**60%** per origin, 80% overall. Other WebKit apps: 15%/20%. Home Screen/Dock web apps get the browser-app quota. Cross-origin frames get 10% of the parent's quota. |
| `persist()` | Heuristic, no prompt | **User prompt** | Heuristic (e.g. Home Screen app), no prompt |
| Eviction | LRU by origin under pressure, non-persistent origins only | same | LRU. **Plus ITP: after 7 days of Safari use with no user interaction, all script-writable storage is deleted** (IndexedDB, LocalStorage, SessionStorage, SW registrations and cache). Home Screen apps "have their own counter of days of use". |
| localStorage | 5 MiB (+5 MiB session) per origin | same | same |

Sources [13]:
- MDN "Storage quotas and eviction criteria" (modified 2026-01-05): https://developer.mozilla.org/en-US/docs/Web/API/Storage_API/Storage_quotas_and_eviction_criteria. The page also contains an inconsistent "10 MiB" sentence.
- WebKit "Updates to Storage Policy" (Safari 17, 2023): https://webkit.org/blog/14403/updates-to-storage-policy/
- WebKit "Full Third-Party Cookie Blocking and More" (2020-03-24): https://webkit.org/blog/10218/full-third-party-cookie-blocking-and-more/

Not confirmed:
- That OPFS is covered by the 7-day rule is **UNVERIFIED**, but assume it is.
- Whether Firefox's `persist()` prompt is now silent on bookmarked or installed sites is UNVERIFIED.

**Other storage facts:**
- **OPFS sync access handle.**
  - `[Exposed=DedicatedWorker, SecureContext]`, with an **exclusive lock** per file.
  - `flush()`: "Attempt to transfer all cached modifications of the file's content to the file system's underlying storage device".
  - `close()` "does not guarantee that all file modifications will be immediately reflected in the underlying storage device. Call the flush() method first" (WHATWG File System LS, updated 2026-10-19, https://fs.spec.whatwg.org/ [13]).
  - Support: Chrome 102 (Android 109), Firefox 111, Safari 15.2. The synchronous method forms need Chrome 108 and Safari 16.4. The `mode` option is Chrome 121 only (BCD).
  - Verified locally (write, flush, getSize = 4) in Chromium 156 and Firefox 159 workers.
  - The chunk/CRC log design is in `rust_math_data.md`.
- **Export.**
  - `showSaveFilePicker`/`showOpenFilePicker` are **Chromium-only**: Chrome 86, Android 132; Firefox and Safari have none (BCD; local Chromium `true`, Firefox `false`).
  - Fallback: build a `Blob` (or `File` from OPFS via `getFile()`) and use `<a download>` with `URL.createObjectURL`.
  - Sessions are tens of MB, so in-memory Blobs are fine. StreamSaver-style streaming is **not needed** (YAGNI). An OPFS `File` is itself a disk-backed Blob, so exporting from OPFS avoids copying into wasm memory.
- **IndexedDB / Cache Storage.** Use IndexedDB only for small settings and index records, if at all (settings fit in localStorage). The Cache Storage API belongs to the SW asset cache only.

---

## 7. Offline / PWA, privacy, secure-context requirements

- **Installability** (MDN, modified 2026-10-06, https://developer.mozilla.org/en-US/docs/Web/Progressive_web_apps/Guides/Making_PWAs_installable [14]):
  - Chromium needs a manifest with `name`/`short_name`, 192 px and 512 px icons, `start_url`, and `display`/`display_override`. It must be served over **https or localhost/127.0.0.1**. A service worker is "not a requirement".
  - Safari 17+ (macOS Sonoma): "Add to Dock" for any site.
  - iOS 16.4+: Share → Add to Home Screen from Safari, Chrome, Edge or Firefox.
  - **Firefox desktop does not support manifest install.** Firefox 143 (Sept 2025) added "pin web app to taskbar" on **Windows only** (secondary: https://www.neowin.net/amp/firefox-143-is-out-with-the-ability-to-pin-sites-to-taskbar-copilot-integration-and-more/ [14]; UNVERIFIED in Mozilla docs).
- **Why install matters for KineForge.** On Safari it moves data out of the 7-day ITP counter and earns the larger quota and `persist()` heuristics. Prompt Safari users to "Add to Dock/Home Screen" before their first long session.
- **Service-worker strategy (minimal).**
  - Precache the hashed `*.wasm`/`*.js`/assets; they are immutable.
  - Use network-first for `index.html`.
  - A new SW **waits**. The UI offers "update available, reload" **only between sessions**, never mid-trial.
  - The SW also injects COOP/COEP when hosted on GitHub Pages (§4).
  - Workbox is optional, not needed.
- **Secure-context requirements** (`localhost`/`127.0.0.1` count as secure):

  | Feature | Requirement |
  |---|---|
  | WebGPU | SecureContext (spec) |
  | OPFS / FileSystemSyncAccessHandle | SecureContext (fs spec) |
  | Service workers and install | https or localhost |
  | SharedArrayBuffer | additionally `crossOriginIsolated` |
  | `pointerrawupdate`, `getCoalescedEvents` | SecureContext (`_browser.md`) |
  | WebHID | SecureContext (`_browser.md` §6.9) |

  A `file://` page is a secure context, but serving wasm and SW from it does not work in practice. **Always use a local HTTP server** (UNVERIFIED per browser).
- **Permissions Policy.** We embed nothing and are embedded nowhere, so `frame-ancestors 'none'` is enough. A WebGPU Permissions-Policy token was **not found**/UNVERIFIED.
- **Privacy defaults.** No network calls after load (enforced by CSP `connect-src 'self'`), no analytics, and an explicit export. `navigator.gpu` adapter info (vendor/architecture) is written into the local session manifest only.

---

## 8. Accessibility

- **Canvas-only UI** (egui on the web) is invisible to screen readers.
  - egui: "For platforms that AccessKit doesn't yet support, including web, there is an experimental built-in screen reader" (speech synthesis) (egui README [13]).
  - AccessKit: web is listed under "Planned adapters" (https://github.com/AccessKit/accesskit README [13]).
  - Keyboard navigation inside egui works (focus and Tab), but it is not exposed to assistive technology.
- **DOM UI** (strategy C) gives native a11y (screen readers, browser zoom, high contrast) for menus, settings and results. The aiming task itself is inherently visual and motor.
- **Pragmatic v1 baseline:**
  - Full keyboard operability of menus.
  - Honour `prefers-reduced-motion` and `prefers-contrast`.
  - Colour-blind-safe target palettes.
  - Scalable UI.
  - Results exportable as CSV/Parquet, which makes them screen-reader-consumable outside the app.
  - A DOM results/settings layer is the upgrade path if accessibility becomes a requirement.

---

## 9. Architecture strategies

| | (A) One Rust codebase incl. wgpu render + egui UI → wasm | (B) Shared Rust core + separate native/web graphics adapters | (C) DOM/TS UI around a wasm canvas (Rust sim + render) | (D) TS/WebGPU renderer, Rust core only |
|---|---|---|---|---|
| Portability | ✅ The same render code runs on DX12/Vulkan/Metal/WebGPU/WebGL2 | ✅ but two renderers to keep visually identical | Render ✅ (A's); UI ✗: native still needs egui ⇒ **two UIs** | Web-only renderer; native needs a Rust one ⇒ two renderers |
| Perf (web) | Good. Main-thread wasm, FIFO/rAF-bound like every web option | Same ceiling | Same, plus DOM layout on the main thread (keep it off during trials) | Same GPU ceiling. JS↔wasm boundary per frame for sim state (small) |
| Bundle | Largest: wgpu + egui (+ naga if WebGL2); measured examples in §3.6 | Can be smaller (e.g. a hand-written WebGL2 path, no naga) | A's wasm without egui (−?) + small TS | **Smallest wasm** (core only), with JS renderer and shaders duplicated |
| Testability | Native unit tests cover sim+render logic. One web smoke suite. wgpu itself is CI-tested on WebGL2 in headless Chromium | Two render test matrices | Plus TS tests (Playwright) | Two renderers, parity tests needed |
| Maintainability | **One language, one shader source (WGSL)**; churn = wgpu/egui majors (~quarterly) | More code, risk of drift | Two UI stacks, TS toolchain (Vite) | Two renderers, two shader dialects if native isn't WGSL |
| Web a11y | ✗ (egui TTS only) | ✗ | ✅ | ✅ if UI is DOM |

**Recommendation: (A), structured so (B), (C) or (D) stay cheap later.**
- `kf-core` (sim, input math, scoring, telemetry format): no wgpu, winit, web-sys or std-only APIs in the hot path. Same `#[test]`s natively and under `wasm-bindgen-test`.
- `kf-render` (wgpu, WebGL2-subset shaders).
- `kf-app` (winit + egui shell).
- `kf-web` (a thin wasm-bindgen entry, the OPFS worker in plain JS, and the SW).

If web a11y or SEO becomes a requirement, add a DOM settings/results page (C) that talks to `kf-web` through a few exported functions. The in-trial canvas stays (A).

### 9.1 Browser CI
- **What wgpu itself does** [13]: the `wasm-test` job on `ubuntu-26.04` uses Playwright Chromium, `headless`, `args: ["--enable-unsafe-swiftshader"]`, with tests built with `--features webgl,exhaust`. **wgpu's own CI exercises the WebGL2 path in the browser, not WebGPU** (https://github.com/gfx-rs/wgpu/blob/trunk/xtask/src/test_wasm.rs, `tests/wasm/runner/index.js`).
- **WebGPU in headless CI (local proof, Chromium 156)**:
  - `chromium --headless=new --enable-unsafe-webgpu --use-webgpu-adapter=swiftshader --enable-features=Vulkan --use-angle=swiftshader` gave adapter `google/swiftshader`, `isFallbackAdapter=true`, `timestamp-query` present, `maxTextureDimension2D=8192`.
  - The flag set matches Chrome's "Supercharge Web AI testing" post (`--use-angle=vulkan --enable-features=Vulkan --disable-vulkan-surface --enable-unsafe-webgpu`; https://developer.chrome.com/blog/supercharge-web-ai-testing [13], search snippet) and Emscripten's CI flags.
  - **Running it on GitHub-hosted runners is UNVERIFIED but likely**, since SwiftShader ships inside Chromium and needs no GPU.
  - A secondary source says headless WebGPU canvases may screenshot black (https://agent-browser.dev/webgpu, UNVERIFIED). ⇒ Assert via `copyTextureToBuffer` readback, not screenshots.
- **Firefox WebGPU cannot be tested on Linux CI** (not shipped). It would need Windows runners, and whether WARP is accepted by the Firefox blocklist is **UNVERIFIED**.
- **Safari WebGPU** needs macOS runners with safaridriver (**UNVERIFIED** in headless).
- **Pure-logic wasm tests:** `wasm-bindgen-test` in headless Chrome/Firefox via WebDriver (standard wasm-bindgen-test runner behaviour; not re-fetched this session ⇒ UNVERIFIED details).
- **Plan:** native `cargo test` with the sim and render logic is the main gate. Web CI = core tests under `wasm-bindgen-test` + one Playwright smoke test (load → adapter, WebGPU-SwiftShader and WebGL2 → render N frames → readback hash → OPFS write and read).

---

## 10. Local checks run on this machine (Chromium 156.0.8078.79, Firefox 159.0; Linux X11; RTX 3070 Ti driver 580 + AMD iGPU)

**Setup.**
- Server: a ~25-line Python `http.server` on `127.0.0.1` serving the probe at `/t.html` with no extra headers, and at `/coi/t.html` with COOP `same-origin` + COEP `require-corp`.
- The probe posts JSON results back to the server.
- Throwaway profiles in the session scratchpad (`--user-data-dir=<scratch>`; Firefox `--no-remote --profile <scratch>` with a 3-pref `user.js`). The user's profiles were not touched.
- First run on 2026-11-13. The Firefox COI run was repeated on 2026-11-14 after the scratchpad reset.

**Commands.**
- `chromium --headless=new --user-data-dir=… URL` (default)
- `… --enable-unsafe-webgpu --enable-features=Vulkan --use-angle=vulkan`
- `… --enable-unsafe-webgpu --ozone-platform=x11 --use-angle=vulkan --enable-features=Vulkan,VulkanFromANGLE` (wiki flags)
- `… --enable-unsafe-webgpu --use-webgpu-adapter=swiftshader --enable-features=Vulkan --use-angle=swiftshader`
- `firefox --headless --no-remote --profile … URL`

**Results.**

| | Chromium default | Chromium `--enable-unsafe-webgpu` + Vulkan | Chromium wiki flags (headless) | Chromium SwiftShader | Firefox 159 |
|---|---|---|---|---|---|
| `navigator.gpu` | yes | yes | yes | yes | **no** |
| `requestAdapter()` | **null** (every option, incl. compat and fallback) | 1st call null; then `{}`→nvidia/ampere, low-power→amd/gcn-5, fallback→swiftshader; ts-query yes; max2D 16384 | null (WebGL2 also failed; headless + `--ozone-platform=x11` likely conflict) | google/swiftshader (fallback), ts-query yes, max2D 8192 | n/a |
| OffscreenCanvas `'webgpu'` in worker | context returned | context returned | — | — | **`GPUCanvasContext` returned despite no `navigator.gpu`** |
| WebGL2 | ANGLE GL NVIDIA, 32768, timer-query ext yes | ANGLE Vulkan NVIDIA | — | ANGLE Vulkan SwiftShader | yes, sanitized renderer, 32768, no timer ext |
| `performance.now` step | 0.1 ms / **0.005 ms COI** | same | — | — | **1 ms** / **0.02 ms COI** |
| `crossOriginIsolated` + SAB under COOP/COEP | true / true | true / true | — | — | true / true |
| wasm validate memory64 / shared memory / simd128 | ✓/✓/✓ | ✓/✓/✓ | — | — | ✓/✓/✓ |
| OPFS sync handle in worker | ✓ | ✓ | — | — | ✓ |
| `showSaveFilePicker` | yes | yes | — | — | no |
| `scheduler.postTask` | yes | yes | — | — | yes |
| rAF (headless) | ~60 Hz (synthetic) | ~60 | — | — | ~61 |

Headed behaviour may differ, especially rAF rate and adapter selection. A headed probe on the 144/240 Hz monitor is an open experiment.

---

## 11. Open questions / experiments

1. **Safari.**
   - Current default rAF cap on ProMotion and external high-refresh displays in Safari 26/27.
   - WebGPU limits and features: is timestamp-query exposed, and how is it quantized?
   - Is OPFS subject to the 7-day ITP purge?
2. **Headed rAF rate** at 144/240/360 Hz in Chrome and Firefox on Windows, macOS, X11 and Wayland, including mixed-refresh multi-monitor setups.
3. **First-`requestAdapter()`-returns-null.** Does it happen in headed or default configurations (e.g. Chrome on Wayland with NVIDIA)? Should KineForge retry before falling back to WebGL2?
4. **Real KineForge wasm size** (gzip and brotli) for webgpu-only vs webgpu+webgl+egui. Brotli availability on Cloudflare and Netlify for `.wasm` is UNVERIFIED.
5. **WebGPU in GitHub Actions** via SwiftShader (expected to work, not run). Firefox/Windows WebGPU on WARP.
6. **Cost of `memory.grow` and JS GC pauses** during a trial (Chrome Performance panel).
7. **Practical per-tab wasm memory limits** on Firefox, Safari and iOS.
8. **Trunk output vs strict CSP**: inline loader script hash, and whether `style-src` needs `'unsafe-inline'` for egui/winit canvas styling.
9. **wgpu 30 device-lost API on web**, and WebGL2 context-loss handling (`webglcontextlost`/`restored`) in wgpu's GLES backend.

---

## 12. Sources
(Inline above. Key ones are listed again with access dates.)
- gpuweb Implementation Status (edited 2026-10-02) — https://github.com/gpuweb/gpuweb/wiki/Implementation-Status [13]
- WebGPU ED 2026-11-10 — https://gpuweb.github.io/gpuweb/ ; TR CRD 2026-09-15 — https://www.w3.org/TR/webgpu/ [13]
- HR-Time ED 2026-11-06 — https://w3c.github.io/hr-time/ [13]
- WHATWG File System LS 2026-10-19 — https://fs.spec.whatwg.org/ [13]
- MDN BCD @69fcf36081 (2026-11-13): `api/{GPU,GPUCanvasContext,OffscreenCanvas,HTMLCanvasElement,WorkerNavigator,FileSystemSyncAccessHandle,FileSystemFileHandle,StorageManager,Window,Scheduler,Performance,Element,PointerEvent,Document}.json`, `http/headers/{Content-Security-Policy,Cross-Origin-Embedder-Policy,Cross-Origin-Opener-Policy}.json`, `javascript/builtins/SharedArrayBuffer.json`, `webassembly/*.json` — https://github.com/mdn/browser-compat-data [13]
- caniuse WebGPU — https://caniuse.com/webgpu [13]
- Chrome: new-in-webgpu-120 / 147-148 / 155-156; developer-features; timer-throttling-in-chrome-88; aligning-input-events; supercharge-web-ai-testing; docs/devtools/wasm — developer.chrome.com [13]/[14]
- Firefox 159 release notes — https://www.firefox.com/en-US/firefox/159.0/releasenotes/ [13]
- WebKit bug 173434 — https://bugs.webkit.org/show_bug.cgi?id=173434 [13]; WebKit blog 14403, 10218 [13]
- MDN: Performance.now, Storage quotas, setTimeout, instantiateStreaming, Making PWAs installable [13]/[14]
- web3dsurvey — https://web3dsurvey.com/webgpu [14]
- wgpu @trunk: README, `wgpu/Cargo.toml`, CHANGELOG, `wgpu/src/util/init.rs`, `wgpu-types/src/limits.rs`, `wgpu-hal/src/gles/adapter.rs`, `.github/workflows/ci.yml`, `xtask/src/test_wasm.rs`, `tests/wasm/runner/index.js` — https://github.com/gfx-rs/wgpu [13]
- Rust: platform-support wasm32-unknown-unknown and wasm64-unknown-unknown; rust#147225; rustwasm sunset post [13]
- wasm-bindgen guide (CLI reference, raytrace example) — https://wasm-bindgen.github.io/wasm-bindgen/ [13]; Trunk assets guide [13]; wasm-bindgen-rayon README [13]; coi-serviceworker README [13]
- Cloudflare Pages headers (updated 2026-11-12), Netlify headers (2025-12-01) [13]
- V8 4 GB wasm memory (2020-05-14) — https://v8.dev/blog/4gb-wasm-memory [14]; SpiderMonkey memory64 (2025-01-15) [13]
- AccessKit README; egui README [13]
- Bevy examples README (size table, July 2022) [13]
- crates.io API (`wgpu, naga, wasm-bindgen(-cli,-futures,-test,-rayon), web-sys, js-sys, egui, eframe, bevy, trunk, wasm-pack, console_error_panic_hook, winit, accesskit, gloo-timers, wasm-opt, web-time`) and GitHub REST API (repo status, releases) [13]
- Deployed artifacts measured: https://www.egui.rs/egui_demo_app_bg.wasm ; https://wgpu.rs/examples/webgpu_bg.wasm ; https://wgpu.rs/examples/webgl2_bg.wasm [13]
