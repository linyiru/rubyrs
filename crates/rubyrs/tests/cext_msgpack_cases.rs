//! L2 acceptance: data-driven msgpack ↔ JSON cross-check using
//! upstream msgpack-ruby's `cases.msg` / `cases.json` corpus.
//!
//! This is the L2 step of the testing-strategy ladder (see
//! docs/TESTING.md): instead of hand-curating fixtures (L1,
//! `cext_msgpack.rs`), we vendor the upstream gem's own
//! data-driven spec corpus and run it. 51 paired entries
//! covering: bool/nil, ints across the full encoding range
//! (fixint, u8/16/32/64, sint, neg fixint), floats, strings
//! (tiny/short/long), arrays/hashes/nested. Each entry is one
//! msgpack-encoded value paired with the equivalent JSON.
//!
//! Test method: parse both files; iterate `Unpacker.read` until
//! the msg buffer empties; element-by-element-compare against
//! the JSON Array via flori/json's parser (already L3-D wedge'd).
//! Equality is structural (rubyrs's `==` on Hash/Array recurses
//! into elements; Float vs nil produces a clean fail signal).
//!
//! Expected pass rate (as of L3-I CValue::Float + binary File.read): 51/51.
//! Pre-L3-I the same corpus showed 47/51 because the 4 Float entries
//! (indices 27-30: 0.0, -0.0, 1.0, -1.0) collapsed to nil — `CValue::Float`
//! didn't exist and `rb_float_new` returned Qnil. The L3-I commit in this
//! PR closes that gap; this floor pins the win so a regression that
//! reintroduces the collapse trips the assertion with a clear per-case
//! diff.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

mod common;

fn ensure_bundles_built() -> (PathBuf, PathBuf) {
    static BUILT: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            // msgpack.bundle (unpacker)
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
            // parser.bundle (flori-json, for parsing the JSON
            // reference file inside the Ruby driver)
            let fj_dir = crate_dir.join("examples/flori-json-cext");
            let fj_build = fj_dir.join("build.sh");
            let fj_out = Command::new("bash")
                .arg(&fj_build)
                .output()
                .expect("failed to spawn flori-json build.sh");
            assert!(
                fj_out.status.success(),
                "flori-json build.sh failed:\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&fj_out.stdout),
                String::from_utf8_lossy(&fj_out.stderr),
            );
            let fj_bundle = fj_dir.join(format!("parser.{}", common::RUBY_DLEXT));
            assert!(fj_bundle.exists(), "missing {}", fj_bundle.display());
            (mp_bundle, fj_bundle)
        })
        .clone()
}

#[test]
fn cext_msgpack_cases_corpus() {
    let (mp_bundle, fj_bundle) = ensure_bundles_built();
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fixture_dir = crate_dir.join("examples/msgpack-cext/fixtures");
    let cases_msg = fixture_dir.join("cases.msg");
    let cases_json = fixture_dir.join("cases.json");
    assert!(cases_msg.exists(), "missing {}", cases_msg.display());
    assert!(cases_json.exists(), "missing {}", cases_json.display());

    // Use a per-test tmp file so parallel test runs don't collide
    // (cf PR #60 review #1).
    let driver_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let driver = driver_dir.join("cext_msgpack_cases_driver.rb");

    // Ruby driver:
    //   1. Read cases.msg as raw bytes (L3-G + File.read raw-byte fix).
    //   2. Read cases.json as text, parse via JSON::Ext::Parser.
    //   3. Unpacker.read in a loop; collect until rescue catches
    //      EOF (msgpack raises when the buffer's exhausted).
    //   4. Side-by-side compare. Print one line per index:
    //        `i: PASS` or `i: FAIL got=X expected=Y`
    //      so the failure summary in the Rust assert message is
    //      readable.
    let script = format!(
        r#"require "{mp}"
require "{fj}"

msg_bytes = File.read("{cases_msg}")
json_text = File.read("{cases_json}")
expected = JSON::Ext::Parser.parse(json_text, {{}})

u = MessagePack::Unpacker.new
u.feed(msg_bytes)
actual = []
done = false
loop_count = 0
last_err_class = nil
last_err_msg = nil
# Loop until buffer empties (rescue terminates the loop).
# Self-review (post-Phase-2 code-review finding F4): the rescue
# is intentionally broad — rubyrs's EOFError class isn't reachable
# from script-level rescue (it would be RuntimeError-wrapped via
# the cext_dispatch sentinel→class fallback), so a class filter
# isn't reliable. To compensate, record the FIRST exception's
# class + message so a future regression that raises something
# unexpected mid-decode shows up in the diagnostic, not buried
# under "case N: FAIL got=nil" attribution.
# `break` from inside a `rescue` body doesn't propagate to the
# enclosing while in rubyrs's current subset (caught while
# writing this test). Use a flag instead.
while loop_count < 200 && !done
  begin
    v = u.read
    actual << v
    loop_count = loop_count + 1
  rescue => e
    last_err_class = e.class.to_s
    last_err_msg = e.message
    done = true
  end
end
if actual.length < 51
  puts "TRUNCATED at i=" + actual.length.to_s +
    " err_class=" + last_err_class.to_s +
    " err_msg=" + last_err_msg.to_s
end

pass = 0
fail = 0
n = [actual.length, expected.length].max
i = 0
while i < n
  a = actual[i]
  e = expected[i]
  if a == e
    puts i.to_s + ": PASS"
    pass = pass + 1
  else
    puts i.to_s + ": FAIL got=" + a.inspect + " expected=" + e.inspect
    fail = fail + 1
  end
  i = i + 1
end
puts "SUMMARY: " + pass.to_s + " pass, " + fail.to_s + " fail (of " + n.to_s + ")"
"#,
        mp = mp_bundle.with_extension("").display(),
        fj = fj_bundle.with_extension("").display(),
        cases_msg = cases_msg.display(),
        cases_json = cases_json.display(),
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

    // Parse the summary. Format: "SUMMARY: P pass, F fail (of N)"
    let summary = stdout
        .lines()
        .find(|l| l.starts_with("SUMMARY:"))
        .unwrap_or_else(|| panic!("no SUMMARY line in stdout:\n{}", stdout));
    let mut iter = summary.split_whitespace();
    iter.next(); // "SUMMARY:"
    let pass: u32 = iter.next().unwrap().parse().expect("pass count");
    iter.next(); // "pass,"
    let fail: u32 = iter.next().unwrap().parse().expect("fail count");

    // Threshold pinned at the current L3-H + binary File.read
    // baseline. Bumping this floor proves a categorical
    // improvement (e.g., adding CValue::Float would jump the
    // floor from 47 to 51 on this corpus).
    //
    // 47 = full corpus (51) minus 4 Float entries (indices 27-30:
    // 0.0, -0.0, 1.0, -1.0) that collapse to nil under rubyrs's
    // current "no CValue::Float; rb_float_new returns Qnil"
    // shape. A regression below 47 means something more
    // fundamental broke.
    //
    // SPEC_STATUS for this corpus tracked in commit message.
    const MIN_PASS: u32 = 51;
    assert!(
        pass >= MIN_PASS,
        "msgpack cases corpus regressed: only {} pass (floor: {}), {} fail.\n\
         Full per-case output:\n{}",
        pass, MIN_PASS, fail, stdout
    );
    // Total entries reachable matches the corpus.
    assert_eq!(
        pass + fail, 51,
        "expected 51 corpus entries, got {} (= {} pass + {} fail).\n{}",
        pass + fail, pass, fail, stdout
    );
}
