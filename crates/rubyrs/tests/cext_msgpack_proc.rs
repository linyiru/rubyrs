//! L3-K acceptance: Proc/Block crossing the cext FFI through
//! `Unpacker#register_type_internal(type_id, klass, proc)`.
//! Pins the end-to-end shape: a Vm-side Proc is passed in to
//! cext code, stored, and later invoked back via
//! `rb_proc_call_with_block` → `rb_funcallv(proc, :call, ...)`
//! → Vm dispatch → Block.call arm → the proc body runs and the
//! result crosses back. Each step has its own host translator
//! arm; this test exercises them all in one shot.
//!
//! The L3-K bridge:
//!   - `CValue::BlockRef(u32)` carries `Value::Block(ObjId).0`
//!     across the FFI. Distinct from `CValue::HeapRef` so the
//!     reverse translator can rebuild `Value::Block` (not
//!     `Value::Object`) on the way back.
//!   - `rb_proc_call_with_block(proc, argc, argv, block)` —
//!     msgpack's `protected_proc_call_safe` reaches for this;
//!     stub forwards through `rb_funcallv` (the `block` arg is
//!     currently ignored; msgpack always passes Qnil).
//!   - `rb_value_type(BlockRef) → T_DATA` + `rb_cProc` sentinel
//!     at handle 20 so `RBASIC_CLASS(proc) == rb_cProc` works.
//!
//! Along the way, three pre-existing cext gaps surfaced:
//!
//!   1. `OBJ_FROZEN(v)` macro was hardcoded to `1` ("conservative:
//!      always-frozen"), which blocked every cext mutation path
//!      that gates on `if (OBJ_FROZEN(self)) rb_raise(FrozenError…)`.
//!      Flipped to `0` to match `rb_obj_frozen_p`'s actual return.
//!
//!   2. `rb_ary_new3(n, ...)` was returning an empty Array
//!      (stable Rust can't take extern "C" variadics). The
//!      msgpack ext-type registries stored
//!      `[ext_module, proc, flags]` triples — empty Array meant
//!      every lookup returned `Qnil` for the proc, surfacing as
//!      `PRIMITIVE_UNEXPECTED_EXT_TYPE` at unpack time. Now
//!      the header redefines `rb_ary_new3` as a variadic macro
//!      that counts `__VA_ARGS__` and dispatches to one of
//!      `rubyrs_ary_new3_1` / `_2` / `_3`.
//!
//!   3. The arity-specialised helpers needed `#[used]` static
//!      references in the rubyrs binary or the linker stripped
//!      them — bundle was dlopen'ing successfully but dlsym
//!      returned NULL stubs at runtime, segfaulting.
//!
//! Wire bytes used: `C7 05 07 68 65 6C 6C 6F` —
//!   `0xC7` (ext8 header) + `0x05` (payload length) + `0x07`
//!   (ext type) + `"hello"` (5-byte payload).

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

mod common;

fn ensure_msgpack_bundle_built() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| common::build_cext_bundle("msgpack-cext", "msgpack")).clone()
}

#[test]
fn cext_msgpack_proc_register_and_invoke() {
    let bundle = ensure_msgpack_bundle_built();
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let driver = driver_dir.join("cext_msgpack_proc_driver.rb");

    // Register a Proc for ext-type 0x07 that wraps the payload
    // bytes; feed pre-built ext-type bytes; verify the Proc was
    // invoked back through the cext → Vm dispatch path.
    let script = format!(
        r#"require "{bundle}"

restorer = proc {{ |s| "RESTORED:" + s }}
u = MessagePack::Unpacker.new
u.register_type_internal(0x07, Object, restorer)
bytes = [0xC7, 0x05, 0x07, 0x68, 0x65, 0x6c, 0x6c, 0x6f].pack("C*")
u.feed(bytes)
got = u.read
puts "got=" + got.inspect
puts "class=" + got.class.name

# Round-trip: a 1-byte-payload ext frame (fixext1 = 0xD4).
short_restorer = proc {{ |s| "SHORT:" + s }}
u2 = MessagePack::Unpacker.new
u2.register_type_internal(0x09, Object, short_restorer)
u2.feed([0xD4, 0x09, 0x41].pack("C*"))
puts "short=" + u2.read.inspect
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

    let want = "got=\"RESTORED:hello\"\nclass=String\nshort=\"SHORT:A\"\n";
    assert_eq!(
        stdout, want,
        "stdout mismatch:\n--- got ---\n{}\n--- want ---\n{}",
        stdout, want,
    );
}
