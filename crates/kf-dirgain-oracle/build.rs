//! Compiles the vendored C reference engine and the layout shim.

fn main() {
    // The oracle links the vendored C into native test binaries; refuse wasm
    // targets (e.g. `--workspace --target wasm32-*`) before the C compiler
    // fails obscurely.
    let family = std::env::var_os("CARGO_CFG_TARGET_FAMILY").unwrap_or_default();
    if family
        .to_str()
        .is_some_and(|f| f.split(',').any(|f| f == "wasm"))
    {
        panic!(
            "kf-dirgain-oracle is native-only; build wasm targets with -p kf-core -p kf-dirgain"
        );
    }
    println!("cargo::rerun-if-changed=c");
    cc::Build::new()
        .files(["c/directional_dpi.c", "c/kf_shim.c"])
        .include("c")
        .std("c11")
        .warnings(true)
        .extra_warnings(true)
        .compile("ddpi_oracle");
}
