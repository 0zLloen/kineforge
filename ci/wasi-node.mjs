// Minimal WASI preview1 runner for `cargo test --target wasm32-wasip1`
// (used as CARGO_TARGET_WASM32_WASIP1_RUNNER by ci/test-wasi.sh).
// Usage: node wasi-node.mjs <module.wasm> [args...]
import { readFile } from "node:fs/promises";
import { argv, env, exit } from "node:process";
import { WASI } from "node:wasi";

const [wasmPath, ...rest] = argv.slice(2);
const wasi = new WASI({ version: "preview1", args: [wasmPath, ...rest], env, returnOnExit: true });
const module = await WebAssembly.compile(await readFile(wasmPath));
const instance = await WebAssembly.instantiate(module, wasi.getImportObject());
exit(wasi.start(instance));
