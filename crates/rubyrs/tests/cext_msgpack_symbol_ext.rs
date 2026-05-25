//! L3-K acceptance, follow-up: Symbol full round-trip via
//! `register_type` ext-type 0x00. Pins the path that needs ALL
//! the L3-J + L3-K bridges plus Class-handle dedup against
//! sentinel slots.
//!
//! End-to-end:
//!   1. Vm-side proc `proc { |s| s.to_s }` crosses to cext via
//!      `CValue::BlockRef` (L3-K).
//!   2. Vm-side `Symbol` Class arg crosses via `CValue::Class`;
//!      the host-side intern path deduplicates against the
//!      seeded sentinel `rb_cSymbol = 10` so the registry's
//!      `if (ext_module == rb_cSymbol) has_symbol_ext_type =
//!      true` branch fires correctly (this commit). Without the
//!      dedup, a fresh handle was being interned and the equality
//!      check failed silently — Symbol values then packed as
//!      fixstr instead of ext-type 0x00.
//!   3. Pack-time: `:foo.to_msgpack_ext`-equivalent proc returns
//!      String "foo"; msgpack's pack path frames it as
//!      `c7 03 00 66 6f 6f` (ext8, len=3, type=0, "foo").
//!   4. Unpack-time: ext-type 0x00 entry's proc is invoked back
//!      through `rb_proc_call_with_block` → `rb_funcallv(:call)`
//!      → Vm dispatch, runs `s.to_sym`, returns `Value::Sym`.
//!   5. Result crosses cext → Vm as `Value::Sym(:foo)`.
//!
//! What we still don't load: msgpack-ruby's pure-Ruby
//! `lib/msgpack/packer.rb` wrapper that turns
//! `register_type(type, klass, :method_name)` into
//! `register_type_internal(type, klass, proc { |o| o.send(method) })`.
//! The fixture hand-rolls that proc.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

mod common;

fn ensure_msgpack_bundle_built() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let mp_dir = crate_dir.join("examples/msgpack-cext");
            let mp_build = mp_dir.join("build.sh");
            let mp_out = Command::new("bash")
                .arg(&mp_build)
                .output()
                .expect("failed to spawn msgpack build.sh");
            assert!(
                mp_out.status.success(),
                "msgpack build.sh failed:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&mp_out.stdout),
                String::from_utf8_lossy(&mp_out.stderr),
            );
            let mp_bundle = mp_dir.join(format!("msgpack.{}", common::RUBY_DLEXT));
            assert!(mp_bundle.exists(), "missing {}", mp_bundle.display());
            mp_bundle
        })
        .clone()
}

#[test]
fn cext_msgpack_symbol_ext_round_trip() {
    let bundle = ensure_msgpack_bundle_built();
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let driver = driver_dir.join("cext_msgpack_symbol_ext_driver.rb");

    // Register Symbol pack/unpack handlers via the internal API
    // and verify round-trip + wire-format byte identity.
    //
    // Expected bytes for `:foo`:
    //   c7  ext8 header
    //   03  payload length (3 bytes)
    //   00  ext-type 0x00 (registered for Symbol)
    //   66 6f 6f  "foo"
    let script = format!(
        r#"require "{bundle}"

to_s_proc = proc {{ |s| s.to_s }}
to_sym_proc = proc {{ |s| s.to_sym }}

p = MessagePack::Packer.new
p.register_type_internal(0x00, Symbol, to_s_proc)
p.write(:foo)
bytes = p.to_str
puts "pack-bytes=" + bytes.bytes.inspect

u = MessagePack::Unpacker.new
u.register_type_internal(0x00, Object, to_sym_proc)
u.feed(bytes)
got = u.read
puts "got=" + got.inspect
puts "class=" + got.class.name

# Round-trip a second symbol to make sure the registry survives
# a re-use (one Packer / Unpacker, multiple writes / reads).
p2 = MessagePack::Packer.new
p2.register_type_internal(0x00, Symbol, to_s_proc)
p2.write(:hello_world)
puts "pack2-bytes=" + p2.to_str.bytes.inspect

u2 = MessagePack::Unpacker.new
u2.register_type_internal(0x00, Object, to_sym_proc)
u2.feed(p2.to_str)
got2 = u2.read
puts "got2=" + got2.inspect
puts "class2=" + got2.class.name
"#,
        bundle = bundle_no_ext.display(),
    );
    fs::write(&driver, script).expect("failed to write driver.rb");

    let rubyrs_bin = env!("CARGO_BIN_EXE_rubyrs");
    let run = Command::new(rubyrs_bin)
        .arg(&driver)
        .output()
        .expect("failed to spawn rubyrs binary");
    let stdout = String::from_utf8_lossy(&run.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&run.stderr).into_owned();
    assert!(
        run.status.success(),
        "rubyrs exited non-zero ({:?}).\nstdout:\n{}\nstderr:\n{}",
        run.status.code(), stdout, stderr,
    );

    // `:foo` → `c7 03 00 'f' 'o' 'o'` = [199, 3, 0, 102, 111, 111]
    // `:hello_world` → `c7 0b 00 'h' 'e' 'l' 'l' 'o' '_' 'w' 'o' 'r' 'l' 'd'`
    //                = [199, 11, 0, 104, 101, 108, 108, 111, 95, 119, 111, 114, 108, 100]
    let want = "pack-bytes=[199, 3, 0, 102, 111, 111]\n\
                got=:foo\n\
                class=Symbol\n\
                pack2-bytes=[199, 11, 0, 104, 101, 108, 108, 111, 95, 119, 111, 114, 108, 100]\n\
                got2=:hello_world\n\
                class2=Symbol\n";
    assert_eq!(
        stdout, want,
        "stdout mismatch:\n--- got ---\n{}\n--- want ---\n{}",
        stdout, want,
    );
}
