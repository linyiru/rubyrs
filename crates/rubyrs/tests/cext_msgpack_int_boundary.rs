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
//!   - Literals BEYOND ±i64::MAX saturate at the rubyrs parser
//!     boundary (master behavior, see ast.rs:as_integer_node).
//!     For example `120938120391283122132313` becomes
//!     `9223372036854775807` (i64::MAX) silently. msgpack's
//!     `Bigint` extension type therefore can't be tested
//!     end-to-end on rubyrs without real Bignum support — and
//!     `MessagePack::Bigint` is pure Ruby (`lib/msgpack/bigint.rb`)
//!     which rubyrs's `require` can't load anyway (it only
//!     handles cext bundles; `.rb` files dlopen-fail as
//!     "not a valid mach-o").
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

    // (value, expected wire-encoded byte count)
    let cases: &[(i64, usize)] = &[
        // fixint range (1 byte each)
        (0, 1),
        (1, 1),
        (-1, 1),
        (127, 1),        // max positive fixint
        (-32, 1),        // min negative fixint
        // u8 / i8 (2 bytes: prefix + 1 byte payload)
        (128, 2),        // just above fixint -> u8
        (-33, 2),        // just below fixint -> i8
        (255, 2),        // u8 max
        (-128, 2),       // i8 min
        // u16 / i16 (3 bytes)
        (256, 3),
        (-129, 3),
        (65535, 3),      // u16 max
        (-32768, 3),     // i16 min
        // u32 / i32 (5 bytes)
        (65536, 5),
        (-32769, 5),
        (4294967295, 5), // u32 max
        (-2147483648, 5),// i32 min
        // u64 / i64 (9 bytes)
        (4294967296, 9),
        (-2147483649, 9),
        (9223372036854775807, 9),   // i64 max
        (-9223372036854775808, 9),  // i64 min
    ];

    // Build a Ruby script that round-trips each case and emits
    // one parseable line per case: "n=V wire=B ok=true/false".
    let mut script = format!("require \"{}\"\n", bundle_no_ext.display());
    for (n, _) in cases {
        script.push_str(&format!(
            r#"
p = MessagePack::Packer.new
p.write({n})
bytes = p.to_str
u = MessagePack::Unpacker.new
u.feed(bytes)
m = u.read
puts "n={n} wire=" + bytes.bytesize.to_s + " ok=" + (m == {n}).to_s
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
    let mut failures: Vec<String> = Vec::new();
    let mut seen = 0;
    for line in stdout.lines() {
        if !line.starts_with("n=") {
            continue;
        }
        // Format: "n=V wire=B ok=true"
        let mut iter = line.split_whitespace();
        let n_part = iter.next().unwrap().trim_start_matches("n=");
        let wire_part = iter.next().unwrap().trim_start_matches("wire=");
        let ok_part = iter.next().unwrap().trim_start_matches("ok=");
        let n: i64 = n_part.parse().expect("parse n");
        let wire: usize = wire_part.parse().expect("parse wire");
        let ok: bool = ok_part == "true";

        let (expected_n, expected_wire) = cases[seen];
        seen += 1;
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
    assert_eq!(
        seen, cases.len(),
        "expected {} lines in stdout, got {}.\nstdout:\n{}",
        cases.len(), seen, stdout,
    );
    assert!(
        failures.is_empty(),
        "msgpack int boundary failures:\n  {}\n\nFull stdout:\n{}",
        failures.join("\n  "),
        stdout,
    );
}
