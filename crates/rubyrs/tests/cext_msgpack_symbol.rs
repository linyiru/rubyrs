//! L3-J acceptance: Symbol crossing the cext FFI through
//! `Packer#write(:foo)` / `Unpacker.read`. Companion to
//! `cext_msgpack.rs` (which covered Int/Str/Array/Hash/Nested)
//! and `cext_msgpack_int_boundary.rs` (full i64 range).
//!
//! What this pins: msgpack-ruby's no-registration Symbol pack
//! path — `Packer#write(:foo)` produces a fixstr `0xA3` + "foo"
//! (matches MRI), and `Unpacker.read` on those bytes returns
//! the String "foo". Same shape as MRI's default behaviour
//! (`MessagePack.pack(:foo)` returns `"\xA3foo"`, unpack gives
//! `"foo"`).
//!
//! The bridge added in L3-J:
//!   - `CValue::Symbol(String)` carries the name across the FFI.
//!   - Vm → cext interns `Value::Sym(id)` into `CValue::Symbol`.
//!   - cext → Vm interns `CValue::Symbol(name)` back to
//!     `Value::Sym`.
//!   - `rb_value_type` returns `T_SYMBOL` (9); `rb_sym2str` /
//!     `rb_id2sym` / `rb_sym2id` go through the existing
//!     thread-local intern table.
//!
//! What's still out of reach (deliberately separate spike):
//!
//!   - `Symbol` round-trip via `Packer#register_type(0x00,
//!     Symbol, :to_msgpack_ext)`. The pure-Ruby wrapper that
//!     normalises `(type, klass, method_sym)` into
//!     `(type, klass, proc { |o| o.send(method) })` lives in
//!     msgpack-ruby's `lib/msgpack/packer.rb`. rubyrs's
//!     `require` only handles cext bundles, so the wrapper
//!     isn't loaded. Calling `register_type_internal` directly
//!     with a Symbol as the 3rd arg fails because that slot
//!     expects a Proc (which doesn't cross the cext FFI yet
//!     either).
//!
//! Wire-encoded sizes asserted from MRI's msgpack output:
//!   `:foo`    → 4 bytes: 0xA3 'f' 'o' 'o'
//!   `:a`      → 2 bytes: 0xA1 'a'
//!   `:`+'x'*31 → 32 bytes: 0xBF + 31 chars (max fixstr)
//!   `:`+'x'*32 → 34 bytes: 0xD9 0x20 + 32 chars (str8)

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
            assert!(
                mp_build.exists(),
                "missing msgpack build.sh at {}",
                mp_build.display(),
            );
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
fn cext_msgpack_symbol_default_pack() {
    let bundle = ensure_msgpack_bundle_built();
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let driver = driver_dir.join("cext_msgpack_symbol_driver.rb");

    // (Ruby literal for the symbol, expected wire bytesize,
    //  expected unpack-result string).
    //
    // Span the fixstr boundary so any off-by-one in the
    // string-length-prefix encoding shows up: max-fixstr is
    // 31 bytes (header `0xBF`), 32+ uses `str8` with a
    // length byte prefix.
    let cases: &[(&str, usize, &str)] = &[
        (":a", 2, "a"),
        (":foo", 4, "foo"),
        (":bar_baz", 8, "bar_baz"),
        // 31-char name — last that fits in fixstr (header A0..BF).
        (
            r#":"abcdefghijklmnopqrstuvwxyz12345""#,
            32,
            "abcdefghijklmnopqrstuvwxyz12345",
        ),
        // 32-char name — promotes to str8 (D9 + 1-byte length + body).
        (
            r#":"abcdefghijklmnopqrstuvwxyz123456""#,
            34,
            "abcdefghijklmnopqrstuvwxyz123456",
        ),
    ];

    // Same begin/rescue-per-case pattern as the sibling int_boundary
    // / cases drivers: a single misbehaving symbol doesn't collapse
    // the whole 5-case matrix into a single opaque non-zero exit.
    let mut script = format!("require \"{}\"\n", bundle_no_ext.display());
    for (i, (lit, _, _expected)) in cases.iter().enumerate() {
        // Expected value is compared on the Rust side via the
        // `got=` field's `inspect` output; keeping it out of the
        // Ruby string avoids double-quoted clashes with the
        // string-symbol literals (`:"abc..."`).
        script.push_str(&format!(
            r#"
begin
  p = MessagePack::Packer.new
  p.write({lit})
  bytes = p.to_str
  u = MessagePack::Unpacker.new
  u.feed(bytes)
  got = u.read
  puts "i={i} wire=" + bytes.bytesize.to_s + " got=" + got.inspect
rescue => e
  puts "i={i} FAIL exception=" + e.class.to_s + ":" + e.message.to_s
end
"#,
            i = i,
            lit = lit,
        ));
    }
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

    let mut failures: Vec<String> = Vec::new();
    let mut seen = 0;
    for line in stdout.lines() {
        if !line.starts_with("i=") {
            continue;
        }
        if seen >= cases.len() {
            failures.push(format!("extra line: {}", line));
            continue;
        }
        let (_lit, want_size, want_str) = cases[seen];
        let want_size_marker = format!("wire={} ", want_size);
        let want_got_marker = format!("got=\"{}\"", want_str);
        if !line.contains(&want_size_marker) || !line.contains(&want_got_marker) {
            failures.push(format!(
                "case {}: expected wire={}, got=\"{}\"; actual line: {}",
                seen, want_size, want_str, line
            ));
        }
        seen += 1;
    }
    assert_eq!(
        seen, cases.len(),
        "expected {} cases, saw {}; stdout:\n{}",
        cases.len(), seen, stdout,
    );
    assert!(
        failures.is_empty(),
        "{} failure(s) across {} cases:\n  - {}\nfull stdout:\n{}",
        failures.len(), cases.len(), failures.join("\n  - "), stdout,
    );
}
