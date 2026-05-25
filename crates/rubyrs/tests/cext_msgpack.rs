//! Spike L3-H acceptance: end-to-end msgpack pack ↔ unpack
//! through the unmodified `msgpack` gem's C extension.
//!
//! Five primitives spread the cext ABI coverage:
//!   - Int  (fixint encoding, 1 byte)
//!   - Str  (fixstr encoding, header + body)
//!   - Array (fixarray + elements)
//!   - Hash with String keys (fixmap + interned keys)
//!   - Nested: Hash containing Array of Hashes
//!
//! Each case validates two properties:
//!
//!   1. **Pack byte-identity with MRI** — rubyrs's
//!      `MessagePack::Packer.new.write(obj).to_str` produces the
//!      EXACT same bytes as MRI + msgpack gem would. Diverging
//!      on a single 0x80+ framing byte means the binary-safe
//!      String path (L3-G) regressed.
//!
//!   2. **Round-trip via Unpacker** — `MessagePack::Unpacker
//!      .new.feed(bytes).read` returns a value `==` to the
//!      original. Requires L3-H (cross-call CExtState lifetime)
//!      so the cext-stored buffer reference survives the
//!      feed → read transition.
//!
//! Reference fixtures (the expected MRI bytes) are inline as hex
//! strings so the test is self-contained — no separate file or
//! generator step. Generated from `ruby -rmsgpack -e
//! 'p MessagePack::Packer.new.write(...).to_str.unpack1("H*")'`
//! against MRI 3.4.1 + msgpack 2.x.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

mod common;

fn ensure_msgpack_bundle_built() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| common::build_cext_bundle("msgpack-cext", "msgpack")).clone()
}

/// Drive a Ruby script via the rubyrs binary; return stdout.
fn run_rubyrs(script: &str, fixture_name: &str) -> String {
    let driver_dir = env!("CARGO_TARGET_TMPDIR");
    let driver = PathBuf::from(driver_dir).join(format!("cext_msgpack_{}.rb", fixture_name));
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
        "rubyrs exited non-zero ({:?}) for fixture {}.\nstdout:\n{}\nstderr:\n{}",
        run.status.code(),
        fixture_name,
        stdout,
        stderr,
    );
    stdout
}

/// MRI-generated expected msgpack bytes (hex) per fixture. Pinned
/// against MRI 3.4.1 + msgpack 2.x — regenerate via:
///   ruby -rmsgpack -e 'p MessagePack::Packer.new.write(OBJ).to_str.unpack1("H*")'
fn fixtures() -> &'static [(&'static str, &'static str, &'static str)] {
    // (name, Ruby literal, MRI hex)
    &[
        ("int",          "42",                                        "2a"),
        ("str",          "\"hello\"",                                 "a568656c6c6f"),
        ("ary",          "[1, 2, 3]",                                 "93010203"),
        ("hash",         "{\"a\" => 1, \"b\" => 2}",                  "82a16101a16202"),
        ("nested_hash",  "{\"users\" => [{\"name\" => \"alice\"}, {\"name\" => \"bob\"}]}",
                         "81a5757365727392 81a46e616d65a5616c69636581a46e616d65a3626f62"),
    ]
}

#[test]
fn cext_msgpack_pack_bytes_match_mri() {
    let bundle = ensure_msgpack_bundle_built();
    let bundle_no_ext = bundle.with_extension("");
    // PR #60 review #1: write fixture output under CARGO_TARGET_TMPDIR
    // (per-test-target dir) instead of a fixed /tmp path. Survives
    // parallel test runs, sandboxed CI, non-Linux environments.
    let tmpdir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));

    for (name, ruby_lit, mri_hex) in fixtures() {
        let out_path = tmpdir.join(format!("cext_msgpack_{}.bin", name));
        // PR #60 review #2: dropped the empty `while i < bytes.length`
        // loop — it was an aborted hex-dump attempt that did nothing
        // useful and obscured the actual test step.
        let script = format!(
            r#"require "{}"
p = MessagePack::Packer.new
p.write({})
bytes = p.to_str
puts "len=#{{bytes.length}}"
File.write("{}", bytes)
"#,
            bundle_no_ext.display(),
            ruby_lit,
            out_path.display(),
        );
        let stdout = run_rubyrs(&script, name);
        // Parse expected hex
        let expected_bytes: Vec<u8> = mri_hex
            .chars()
            .filter(|c| c.is_ascii_hexdigit())
            .collect::<Vec<_>>()
            .chunks(2)
            .map(|c| u8::from_str_radix(&c.iter().collect::<String>(), 16).unwrap())
            .collect();
        let expected_len = expected_bytes.len();
        assert!(
            stdout.contains(&format!("len={}", expected_len)),
            "fixture {}: expected len {} not in stdout\nstdout:\n{}",
            name, expected_len, stdout
        );
        let actual_bytes = fs::read(&out_path)
            .unwrap_or_else(|e| panic!("can't read {}: {}", out_path.display(), e));
        assert_eq!(
            actual_bytes, expected_bytes,
            "fixture {} (Ruby: {}): bytes don't match MRI\n  expected hex: {}\n  got hex:      {}",
            name, ruby_lit, mri_hex.replace(' ', ""),
            actual_bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>(),
        );
    }
}

#[test]
fn cext_msgpack_round_trip_via_unpacker() {
    let bundle = ensure_msgpack_bundle_built();
    let bundle_no_ext = bundle.with_extension("");

    // Ruby-level round-trip: pack obj, unpack the resulting bytes,
    // compare equality with original. Exercises the L3-H
    // cross-call lifetime: feed and read are separate cext
    // dispatches, so the bytes VALUE handle msgpack stores in
    // the Unpacker's TypedData must survive the leave/enter
    // boundary.
    let mut script = format!(
        r#"require "{}"
"#,
        bundle_no_ext.display()
    );
    for (name, ruby_lit, _) in fixtures() {
        script.push_str(&format!(
            r#"obj = {}
p = MessagePack::Packer.new
p.write(obj)
bytes = p.to_str
u = MessagePack::Unpacker.new
u.feed(bytes)
r = u.read
puts "{}: " + (obj == r).to_s + " | got=" + r.inspect
"#,
            ruby_lit, name
        ));
    }
    let stdout = run_rubyrs(&script, "round_trip");
    for (name, ruby_lit, _) in fixtures() {
        let needle = format!("{}: true", name);
        assert!(
            stdout.contains(&needle),
            "fixture {} (Ruby: {}): round-trip failed\nstdout:\n{}",
            name, ruby_lit, stdout
        );
    }
}
