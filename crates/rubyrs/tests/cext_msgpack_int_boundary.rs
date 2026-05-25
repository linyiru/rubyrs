//! L2 follow-up: msgpack integer wire-format coverage across the
//! full i64 range. Companion to `cext_msgpack_cases.rs` (which
//! covers the upstream sample but only goes up to u32 / down to
//! i32 in the int columns).
//!
//! What this pins: 21 boundary values spanning every msgpack int
//! encoding width — fixint (1 byte), u8/i8, u16/i16, u32/i32,
//! u64/i64 — all round-trip self-consistently through
//! `Packer.write(n).to_str → Unpacker.feed(bytes).read`.
//!
//! What this does NOT cover (documented gap):
//!
//!   - Literals BEYOND the i64 range saturate at the rubyrs
//!     parser boundary by sign (master behavior, see
//!     `crates/rubyrs/src/ast.rs` integer-literal translation):
//!     positive overflow clamps to i64::MAX, negative overflow
//!     clamps to i64::MIN. For example `120938120391283122132313`
//!     becomes `9223372036854775807` (i64::MAX) silently, and
//!     `-120938120391283122132313` becomes `-9223372036854775808`
//!     (i64::MIN) silently. msgpack's `Bigint` extension type
//!     therefore can't be tested end-to-end on rubyrs without
//!     real Bignum support — and `MessagePack::Bigint` is pure
//!     Ruby (`lib/msgpack/bigint.rb`) which rubyrs's `require`
//!     can't load anyway (it only handles cext bundles; passing
//!     a `.rb` file to `require` dlopen-fails because it isn't
//!     a shared library — the exact error text varies by host
//!     ("not a valid mach-o file" on macOS, "invalid ELF
//!     header" / "file too short" on Linux).
//!
//!   - msgpack-ruby's `bigint_spec.rb` is blocked on BOTH gaps
//!     above; see commit message + SUBSET.md note for the full
//!     finding chain. This test is the closest L2 deliverable
//!     achievable without those features.
//!
//! Wire-encoded sizes asserted (from msgpack-ruby's own corpus
//! and the spec):
//!   1 byte:  -32 .. 127  (fixint)
//!   2 bytes: u8 / i8 ranges outside fixint
//!   3 bytes: u16 / i16
//!   5 bytes: u32 / i32
//!   9 bytes: u64 / i64

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
fn cext_msgpack_int_boundary_round_trip() {
    let bundle = ensure_msgpack_bundle_built();
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let driver = driver_dir.join("cext_msgpack_int_boundary_driver.rb");

    // (value, expected wire-encoded byte count).
    // Boundary values named via constants where possible — i64::MAX
    // / i64::MIN / u32::MAX / etc. read more obviously as "type
    // boundary" than 19-digit decimals and remove transcription risk.
    let cases: &[(i64, usize)] = &[
        // fixint range (1 byte each)
        (0, 1),
        (1, 1),
        (-1, 1),
        (127, 1),                        // max positive fixint
        (-32, 1),                        // min negative fixint
        // u8 / i8 (2 bytes: prefix + 1 byte payload)
        (128, 2),                        // just above fixint -> u8
        (-33, 2),                        // just below fixint -> i8
        (u8::MAX as i64, 2),             // u8 max  (255)
        (i8::MIN as i64, 2),             // i8 min  (-128)
        // u16 / i16 (3 bytes)
        (u8::MAX as i64 + 1, 3),         // 256
        (i8::MIN as i64 - 1, 3),         // -129
        (u16::MAX as i64, 3),            // 65535
        (i16::MIN as i64, 3),            // -32768
        // u32 / i32 (5 bytes)
        (u16::MAX as i64 + 1, 5),        // 65536
        (i16::MIN as i64 - 1, 5),        // -32769
        (u32::MAX as i64, 5),            // 4294967295
        (i32::MIN as i64, 5),            // -2147483648
        // u64 / i64 (9 bytes)
        (u32::MAX as i64 + 1, 9),        // 4294967296
        (i32::MIN as i64 - 1, 9),        // -2147483649
        (i64::MAX, 9),                   // 9223372036854775807
        (i64::MIN, 9),                   // -9223372036854775808
    ];

    // Build a Ruby script that round-trips each case and emits
    // one parseable line per case: "n=V wire=B ok=true/false" on
    // success, or "n=V FAIL exception=<class>:<message>" on raise.
    //
    // Reviewer finding F2 (post-PR-#68): wrap each case in
    // begin/rescue so a single failing case does NOT abort the
    // whole driver — without this, one regression collapses the
    // 21-case matrix into a single opaque "rubyrs exited
    // non-zero" with no per-case attribution, defeating the
    // diagnostic the docstring promises. Mirrors the rescue
    // pattern in sibling `cext_msgpack_cases.rs` lines 126-136.
    let mut script = format!("require \"{}\"\n", bundle_no_ext.display());
    for (n, _) in cases {
        script.push_str(&format!(
            r#"
begin
  p = MessagePack::Packer.new
  p.write({n})
  bytes = p.to_str
  u = MessagePack::Unpacker.new
  u.feed(bytes)
  m = u.read
  puts "n={n} wire=" + bytes.bytesize.to_s + " ok=" + (m == {n}).to_s
rescue => e
  puts "n={n} FAIL exception=" + e.class.to_s + ":" + e.message.to_s
end
"#,
            n = n
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

    // Parse each line, compare against expected wire size + ok.
    // Reviewer finding F3 (post-PR-#68): bound-check before
    // indexing `cases[seen]` — if rubyrs ever emits an extra
    // `n=`-prefixed line (regression / debug print), the loop's
    // 22nd iteration would otherwise panic with an opaque
    // "index out of bounds" and short-circuit the clean
    // `assert_eq!(seen, cases.len(), ...)` diagnostic below.
    // Now extras are recorded as failures so the downstream
    // diagnostic still produces a useful message.
    let mut failures: Vec<String> = Vec::new();
    let mut seen = 0;
    for line in stdout.lines() {
        if !line.starts_with("n=") {
            continue;
        }
        if seen >= cases.len() {
            // Don't advance `seen` past `cases.len()` for extras;
            // the post-loop `assert_eq!(seen, cases.len(), ...)`
            // would otherwise fire BEFORE the `failures.is_empty()`
            // assertion and swallow the per-line diagnostic
            // (reviewer Copilot finding on PR #68). Track extras
            // exclusively through `failures` so the assertions
            // fire in the right order: per-line failures first,
            // then a count mismatch only if extras were 0 but
            // some case was skipped.
            failures.push(format!("unexpected extra line: {}", line));
            continue;
        }
        let (expected_n, expected_wire) = cases[seen];
        seen += 1;

        // Per-case FAIL (rescued exception in driver) — surface
        // the class:message verbatim with the case index.
        if let Some(rest) = line.strip_prefix(&format!("n={} FAIL exception=", expected_n)) {
            failures.push(format!(
                "case {}: n={} raised — {}",
                seen, expected_n, rest
            ));
            continue;
        }

        // Format: "n=V wire=B ok=true". Reject malformed lines
        // explicitly rather than unwrapping mid-parse.
        let toks: Vec<&str> = line.split_whitespace().collect();
        if toks.len() != 3 {
            failures.push(format!(
                "case {}: malformed line ({} tokens, expected 3): {:?}",
                seen, toks.len(), line
            ));
            continue;
        }
        let n_part = toks[0].trim_start_matches("n=");
        let wire_part = toks[1].trim_start_matches("wire=");
        let ok_part = toks[2].trim_start_matches("ok=");
        let n: i64 = match n_part.parse() {
            Ok(v) => v,
            Err(_) => {
                failures.push(format!(
                    "case {}: cannot parse n from {:?}",
                    seen, line
                ));
                continue;
            }
        };
        let wire: usize = match wire_part.parse() {
            Ok(v) => v,
            Err(_) => {
                failures.push(format!(
                    "case {}: cannot parse wire from {:?}",
                    seen, line
                ));
                continue;
            }
        };
        let ok: bool = ok_part == "true";

        if n != expected_n {
            failures.push(format!(
                "case {}: n mismatch — got {}, expected {}",
                seen, n, expected_n
            ));
            continue;
        }
        if wire != expected_wire {
            failures.push(format!(
                "case {}: n={} wire mismatch — got {} bytes, expected {}",
                seen, n, wire, expected_wire
            ));
        }
        if !ok {
            failures.push(format!(
                "case {}: n={} did NOT round-trip (m != n)",
                seen, n
            ));
        }
    }
    // Failures first — per-line diagnostics are more actionable
    // than a bare count mismatch, and a malformed-line failure
    // typically explains why the count also went wrong.
    assert!(
        failures.is_empty(),
        "msgpack int boundary failures:\n  {}\n\nFull stdout:\n{}",
        failures.join("\n  "),
        stdout,
    );
    assert_eq!(
        seen, cases.len(),
        "expected {} lines in stdout, got {}.\nstdout:\n{}",
        cases.len(), seen, stdout,
    );
}
