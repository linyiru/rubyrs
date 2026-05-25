//! Spike L3-C wedge acceptance: end-to-end JSON parse + generate
//! through a C extension that exercises (in one go) every L2/L3
//! cext primitive we've shipped:
//!
//!   - rb_define_module + rb_define_singleton_method
//!   - rb_str_new / RSTRING_PTR / RSTRING_LEN
//!   - rb_ary_new / rb_ary_push / rb_ary_entry / RARRAY_LEN
//!   - rb_hash_new / rb_hash_aset
//!   - rb_long2num / NUM2LONG
//!   - rb_intern + rb_funcallv / rb_funcall (variadic)
//!   - rb_raise(rb_eArgumentError, ...)  with vsnprintf fmt args
//!   - Qnil / Qtrue / Qfalse pass-through
//!
//! Together these are the load-bearing surface a real-world C
//! extension (like flori/json or other "wrap a parser, return
//! Ruby objects" gems) needs. Passing this test is the smallest
//! useful proof that the cext FFI is complete enough for that
//! class of gem.
//!
//! Scope is deliberately small: no escapes inside strings, no
//! floats, no unicode. Real flori/json vendoring is L3-D.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

fn ensure_mini_json_bundle_built() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
            let example_dir = crate_dir.join("examples/mini-json-cext");
            let build_sh = example_dir.join("build.sh");
            assert!(build_sh.exists(), "missing build.sh at {}", build_sh.display());
            let build = Command::new("bash")
                .arg(&build_sh)
                .output()
                .expect("failed to spawn build.sh");
            assert!(
                build.status.success(),
                "build.sh failed.\nstdout:\n{}\nstderr:\n{}",
                String::from_utf8_lossy(&build.stdout),
                String::from_utf8_lossy(&build.stderr),
            );
            let ext = if cfg!(target_os = "macos") { "bundle" }
                      else if cfg!(windows) { "dll" }
                      else { "so" };
            let bundle = example_dir.join(format!("mini_json.{}", ext));
            assert!(bundle.exists(), "build.sh did not produce {}", bundle.display());
            bundle
        })
        .clone()
}

#[test]
fn cext_mini_json_parse_and_generate() {
    let bundle = ensure_mini_json_bundle_built();
    let bundle_no_ext = bundle.with_extension("");
    let driver_dir = env!("CARGO_TARGET_TMPDIR");
    let driver = PathBuf::from(driver_dir).join("cext_mini_json_driver.rb");
    fs::write(
        &driver,
        format!(
            r#"require "{}"

# 1. Parse: flat Array of Int.
p MiniJson.parse("[1,2,3]")

# 2. Parse: nested Object with Array of mixed primitives.
p MiniJson.parse(%q({{"a":1,"b":[true,false,null]}}))

# 3. Generate: round-trip the same nested shape.
puts MiniJson.generate([1, 2, "x"])
puts MiniJson.generate({{"k" => 42}})

# 4. Parse error path: rb_raise(rb_eArgumentError, fmt, ...)
#    surfaces as a Ruby-side ArgumentError with the formatted
#    message. Proves L3-A wired through a real parser context.
begin
  MiniJson.parse("[1,")
  puts "fail: no raise"
rescue ArgumentError => e
  puts "rescued: #{{e.message}}"
end
"#,
            bundle_no_ext.display()
        ),
    )
    .expect("failed to write driver.rb");

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
        run.status.code(),
        stdout,
        stderr,
    );

    let expected = "\
[1, 2, 3]
{\"a\" => 1, \"b\" => [true, false, nil]}
[1,2,\"x\"]
{\"k\":42}
rescued: unexpected end of input
";

    assert_eq!(
        stdout, expected,
        "mini-json round trip mismatch.\n\
         expected:\n{}\n\
         got:\n{}\n\
         stderr:\n{}",
        expected, stdout, stderr,
    );
}
