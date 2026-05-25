// Compile the setjmp / longjmp shim used by Spike L3-A (rb_raise).
//
// See c/setjmp_shim.c's header comment for why the shim lives in C
// rather than Rust. The short version: setjmp's captured context
// is bound to the C frame that called it; trying to setjmp in a
// Rust frame and longjmp from a different Rust frame skips RAII
// drops and is implementation-defined at best.
//
// Excluded on wasm32-wasi: longjmp emulation there is flaky and
// the cext path is already wasi-stubbed in vm.rs anyway.

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.starts_with("wasm32-wasi") {
        return;
    }

    cc::Build::new()
        .file("c/setjmp_shim.c")
        .warnings(true)
        .extra_warnings(true)
        .compile("rubyrs_setjmp_shim");

    println!("cargo:rerun-if-changed=c/setjmp_shim.c");
    println!("cargo:rerun-if-changed=build.rs");
}
