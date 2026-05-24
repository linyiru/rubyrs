fn main() {
    // For wasm32-wasip1, Rust std references __wasi_init_tp (thread-pool init)
    // even when we don't use threads. Provide a no-op stub from a tiny C file.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.starts_with("wasm32-wasi") {
        let stub_path = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("wasi_stub.c");
        std::fs::write(&stub_path, "void __wasi_init_tp(void) {}\n").unwrap();
        cc::Build::new()
            .file(&stub_path)
            .compile("wasi_stub");
        println!("cargo:rerun-if-changed=build.rs");
    }
}
