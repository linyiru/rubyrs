// A Ruby loadable extension resolves the `rb_*` symbols at `require` time
// from the host Ruby process, not at link time. On macOS that needs
// `-undefined dynamic_lookup`; on Linux the dynamic linker resolves them by
// default. (A real gem build delegates this to rb-sys/rake-compiler; this
// keeps a plain `cargo build` producing a loadable object for dev/bench.)
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-undefined,dynamic_lookup");
    }
}
