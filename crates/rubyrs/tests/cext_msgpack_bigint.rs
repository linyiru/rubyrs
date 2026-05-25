//! A6d acceptance: load msgpack-ruby's vendored
//! `lib/msgpack/bigint.rb` and verify `to_msgpack_ext` /
//! `from_msgpack_ext` round-trip byte-identical to MRI for
//! values in the i64 range.
//!
//! Why not a diff_cruby fixture: CRuby resolves
//! `MessagePack::Bigint` through proper nested-module
//! constant lookup; the rubyrs Tier 1 subset flattens nested
//! `module Foo; module Bar; … end; end` so `Bar` ends up at
//! top-level and `Foo::Bar` returns nil (separate
//! nested-namespacing gap, NOT in A6 scope). A diff_cruby
//! fixture written with `MessagePack::Bigint.to_msgpack_ext`
//! would diverge structurally even though the wire output
//! is identical. This Rust integration test exercises the
//! workaround (top-level `Bigint` access) and asserts the
//! actual byte sequence — the contract that matters for
//! protocol compat.
//!
//! Scope (Tier 1 protocol-compat only):
//!   - Inputs MUST be in i64 range. Anything outside has
//!     already saturated at the parser by the time bigint.rb
//!     sees the value (D10 commit; documented in SUBSET.md
//!     and in ADR 0015's Tier 2 BigInt assignment).
//!   - The byte-sequence assertion is against MRI's actual
//!     output for the same input, captured beforehand and
//!     committed inline as expected values. Re-run MRI with
//!     `ruby -rmsgpack -e 'p MessagePack::Bigint.to_msgpack_ext(N).bytes'`
//!     to refresh.
//!
//! What this proves end-to-end:
//!   1. `require ".../bigint.rb"` works (A5).
//!   2. `Integer.instance_method(:[]).arity != 1` resolves
//!      without NameError (A6c).
//!   3. `bigint[offset, length]` extracts the right bits
//!      (A6b).
//!   4. `Array#pack("CL>*")` produces correct BE u32 limbs
//!      (A6a).
//!   5. `Array#unpack("CL>*")` reverses the pack (A6a + D9).
//!   6. `<<` accumulator + `Integer#+` arithmetic on i64
//!      drives `from_msgpack_ext`.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn cext_msgpack_bigint_i64_range_round_trip() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let bigint_rb = crate_dir.join("examples/msgpack-cext/vendor-rb/msgpack/bigint.rb");
    assert!(
        bigint_rb.exists(),
        "missing vendored bigint.rb at {}",
        bigint_rb.display()
    );

    // Values + expected MRI byte sequences (captured from
    // `ruby -rmsgpack -e 'p MessagePack::Bigint.to_msgpack_ext(N).bytes'`).
    //
    // Each entry is (value, expected_pack_bytes).
    // Sign tag (1 byte) + (bit_length / 32 rounded up) × 4 bytes BE.
    let cases: &[(i64, &[u8])] = &[
        // Single 32-bit chunk values. (n=0 is a special case:
        // `bit_length` returns 0 so the while-loop never runs,
        // producing just the sign byte. Matches MRI.)
        (0,                   &[0]),
        (1,                   &[0, 0, 0, 0, 1]),
        (-1,                  &[1, 0, 0, 0, 1]),
        (255,                 &[0, 0, 0, 0, 255]),
        (i32::MAX as i64,     &[0, 127, 255, 255, 255]),
        // Negative tag (1) + magnitude as 32-bit BE.
        (-(i32::MAX as i64),  &[1, 127, 255, 255, 255]),
        // 64-bit values — two 32-bit chunks, LSB chunk first.
        (0x123456789ABCDEF0,  &[0, 154, 188, 222, 240, 18, 52, 86, 120]),
        (i64::MAX,            &[0, 255, 255, 255, 255, 127, 255, 255, 255]),
        // i64::MIN — magnitude can't be represented as positive i64,
        // so the `bigint = -bigint` step inside to_msgpack_ext
        // would overflow. Skip — documented edge in SUBSET.md.
    ];

    let driver_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let driver = driver_dir.join("cext_msgpack_bigint_driver.rb");

    // Drive the round-trip per case. Access Bigint at top-level
    // (the nested-module workaround). For each value:
    //   1. pack via Bigint.to_msgpack_ext
    //   2. print the resulting bytes
    //   3. unpack via Bigint.from_msgpack_ext
    //   4. assert equal to original
    let mut script = format!(
        "require \"{}\"\n",
        bigint_rb.display()
    );
    for (i, (n, _)) in cases.iter().enumerate() {
        script.push_str(&format!(
            r#"
begin
  v = {n}
  bytes = Bigint.to_msgpack_ext(v)
  back  = Bigint.from_msgpack_ext(bytes)
  puts "i={i} bytes=" + bytes.bytes.inspect + " back=" + back.to_s + " match=" + (back == v).to_s
rescue => e
  puts "i={i} FAIL " + e.class.to_s + ":" + e.message
end
"#,
            n = n, i = i,
        ));
    }
    fs::write(&driver, script).expect("failed to write driver");

    let rubyrs = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs)
        .arg(&driver)
        .output()
        .expect("failed to spawn rubyrs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "rubyrs exited non-zero ({:?}).\nstdout:\n{}\nstderr:\n{}",
        out.status.code(), stdout, stderr,
    );

    // Parse one line per case; verify byte sequence + match=true.
    let lines: Vec<&str> = stdout.lines().filter(|l| l.starts_with("i=")).collect();
    assert_eq!(
        lines.len(), cases.len(),
        "expected {} cases, got {} lines.\nstdout:\n{}",
        cases.len(), lines.len(), stdout,
    );

    let mut failures: Vec<String> = Vec::new();
    for (i, (n, expected_bytes)) in cases.iter().enumerate() {
        let line = lines[i];
        let expected_bytes_str = format!("{:?}", expected_bytes);
        let want_bytes = format!("bytes={}", expected_bytes_str);
        let want_match = "match=true";
        if !line.contains(&want_bytes) || !line.contains(want_match) {
            failures.push(format!(
                "case {} (n={}): want `{}` AND `{}`; got line: {}",
                i, n, want_bytes, want_match, line,
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} case(s) failed:\n  - {}\nfull stdout:\n{}",
        failures.len(), failures.join("\n  - "), stdout,
    );
}
