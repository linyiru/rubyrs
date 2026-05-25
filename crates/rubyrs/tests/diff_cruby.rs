//! Differential testing against CRuby.
//!
//! Each `tests/diff/*.rb` is executed under both rubyrs and the system
//! `ruby` interpreter; stdout must match byte-for-byte. CRuby acts as
//! the oracle: any deviation is a rubyrs bug (or, rarely, an
//! intentionally documented divergence — see SUBSET.md).
//!
//! If `ruby` is not on PATH, tests skip with a warning rather than fail,
//! so `cargo test` works on machines without CRuby. CI is expected to
//! provide Ruby; both ubuntu-latest and macos-latest images ship with
//! it pre-installed.

use std::path::PathBuf;
use std::process::Command;

fn rubyrs_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_rubyrs"))
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn ruby_available() -> bool {
    Command::new("ruby")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_diff(name: &str) {
    if !ruby_available() {
        eprintln!("skipping diff_cruby::{} — `ruby` not on PATH", name);
        return;
    }
    let dir = manifest_dir().join("tests/diff");
    let rb_rel = PathBuf::from("tests/diff").join(format!("{name}.rb"));
    let rb_abs = dir.join(format!("{name}.rb"));
    assert!(rb_abs.exists(), "missing diff fixture: {}", rb_abs.display());

    let ours = Command::new(rubyrs_bin())
        .current_dir(manifest_dir())
        .arg(&rb_rel)
        .output()
        .expect("failed to spawn rubyrs");
    let theirs = Command::new("ruby")
        .arg("--disable=gems")
        .current_dir(manifest_dir())
        .arg(&rb_rel)
        .output()
        .expect("failed to spawn ruby");

    assert!(
        theirs.status.success(),
        "CRuby itself failed on {} (probably a fixture bug):\n{}",
        name,
        String::from_utf8_lossy(&theirs.stderr)
    );
    assert!(
        ours.status.success(),
        "rubyrs failed on {} but CRuby succeeded:\nstderr:\n{}",
        name,
        String::from_utf8_lossy(&ours.stderr)
    );

    let ours_stdout = String::from_utf8_lossy(&ours.stdout);
    let theirs_stdout = String::from_utf8_lossy(&theirs.stdout);
    assert_eq!(
        ours_stdout, theirs_stdout,
        "stdout mismatch for {}:\n--- rubyrs:\n{}\n--- CRuby:\n{}",
        name, ours_stdout, theirs_stdout,
    );
}

#[test] fn integer_basics() { run_diff("integer_basics"); }
#[test] fn string_basics() { run_diff("string_basics"); }
#[test] fn array_basics() { run_diff("array_basics"); }
#[test] fn hash_basics() { run_diff("hash_basics"); }
#[test] fn block_basics() { run_diff("block_basics"); }
#[test] fn class_basics() { run_diff("class_basics"); }
#[test] fn symbol_basics() { run_diff("symbol_basics"); }
#[test] fn interpolation() { run_diff("interpolation"); }
#[test] fn rescue_basics() { run_diff("rescue_basics"); }
#[test] fn fizzbuzz_15() { run_diff("fizzbuzz_15"); }
#[test] fn inheritance() { run_diff("inheritance"); }
#[test] fn custom_exception() { run_diff("custom_exception"); }
#[test] fn ensure_basics() { run_diff("ensure_basics"); }
#[test] fn control_flow() { run_diff("control_flow"); }
#[test] fn range_basics() { run_diff("range_basics"); }
#[test] fn enumerable_filter() { run_diff("enumerable_filter"); }
#[test] fn enumerable_aggregate() { run_diff("enumerable_aggregate"); }
#[test] fn int_string_basics() { run_diff("int_string_basics"); }
#[test] fn array_extras() { run_diff("array_extras"); }
#[test] fn hash_extras() { run_diff("hash_extras"); }
#[test] fn rescue_by_class() { run_diff("rescue_by_class"); }
#[test] fn default_args() { run_diff("default_args"); }
#[test] fn respond_to() { run_diff("respond_to"); }
#[test] fn object_class() { run_diff("object_class"); }
#[test] fn cross_type_eq() { run_diff("cross_type_eq"); }
#[test] fn float_basics() { run_diff("float_basics"); }
#[test] fn attr_accessor() { run_diff("attr_accessor"); }
#[test] fn spaceship() { run_diff("spaceship"); }
#[test] fn string_transform() { run_diff("string_transform"); }
#[test] fn int_bits() { run_diff("int_bits"); }
#[test] fn enumerable_by() { run_diff("enumerable_by"); }
#[test] fn super_call() { run_diff("super_call"); }
#[test] fn return_nonlocal() { run_diff("return_nonlocal"); }
#[test] fn methods_batch() { run_diff("methods_batch"); }
#[test] fn rescue_primitive() { run_diff("rescue_primitive"); }
#[test] fn zero_division() { run_diff("zero_division"); }
#[test] fn multi_write() { run_diff("multi_write"); }
#[test] fn splat_multi_write() { run_diff("splat_multi_write"); }
#[test] fn string_format() { run_diff("string_format"); }
#[test] fn array_zip() { run_diff("array_zip"); }
#[test] fn comparable() { run_diff("comparable"); }
#[test] fn op_assign() { run_diff("op_assign"); }
#[test] fn range_enumerable() { run_diff("range_enumerable"); }
#[test] fn string_search() { run_diff("string_search"); }
#[test] fn visibility() { run_diff("visibility"); }
#[test] fn raise_two_arg() { run_diff("raise_two_arg"); }
#[test] fn user_sort() { run_diff("user_sort"); }
#[test] fn hash_enumerable() { run_diff("hash_enumerable"); }
#[test] fn kernel_p() { run_diff("kernel_p"); }
#[test] fn string_slice() { run_diff("string_slice"); }
#[test] fn power() { run_diff("power"); }
#[test] fn block_autosplat() { run_diff("block_autosplat"); }
#[test] fn unless_until() { run_diff("unless_until"); }
#[test] fn string_assign() { run_diff("string_assign"); }
#[test] fn inline_rescue() { run_diff("inline_rescue"); }
#[test] fn method_name() { run_diff("method_name"); }
#[test] fn inspect_orphans() { run_diff("inspect_orphans"); }
#[test] fn symbol_to_proc() { run_diff("symbol_to_proc"); }
#[test] fn case_when() { run_diff("case_when"); }
#[test] fn modules() { run_diff("modules"); }
#[test] fn conversions() { run_diff("conversions"); }
#[test] fn unless_basics() { run_diff("unless_basics"); }
#[test] fn regex_minimal() { run_diff("regex_minimal"); }
#[test] fn lambdas() { run_diff("lambdas"); }
#[test] fn string_mutation() { run_diff("string_mutation"); }
#[test] fn defined() { run_diff("defined"); }
#[test] fn array_bang() { run_diff("array_bang"); }
#[test] fn percent_literals() { run_diff("percent_literals"); }
#[test] fn frozen_strings() { run_diff("frozen_strings"); }
#[test] fn splat_calls() { run_diff("splat_calls"); }
#[test] fn keyword_args() { run_diff("keyword_args"); }
#[test] fn env_hash() { run_diff("env_hash"); }
#[test] fn match_data() { run_diff("match_data"); }
#[test] fn file_io() { run_diff("file_io"); }
#[test] fn range_extras() { run_diff("range_extras"); }
#[test] fn tap_then() { run_diff("tap_then"); }
#[test] fn enumerable_advanced() { run_diff("enumerable_advanced"); }
#[test] fn dig() { run_diff("dig"); }
#[test] fn ternary() { run_diff("ternary"); }
#[test] fn def_self_method() { run_diff("def_self_method"); }
#[test] fn constant_write() { run_diff("constant_write"); }
#[test] fn block_destructure() { run_diff("block_destructure"); }
#[test] fn range_strings() { run_diff("range_strings"); }
#[test] fn kwrest_args() { run_diff("kwrest_args"); }
#[test] fn case_splat() { run_diff("case_splat"); }
#[test] fn nonlocal_return() { run_diff("nonlocal_return"); }
#[test] fn lambda_builtin() { run_diff("lambda_builtin"); }
#[test] fn hash_inspect_quotes() { run_diff("hash_inspect_quotes"); }
#[test] fn anon_kwrest() { run_diff("anon_kwrest"); }
#[test] fn block_destructure_mixed() { run_diff("block_destructure_mixed"); }
#[test] fn block_destructure_nested() { run_diff("block_destructure_nested"); }
#[test] fn module_include() { run_diff("module_include"); }
#[test] fn default_args_exprs() { run_diff("default_args_exprs"); }
#[test] fn block_arg_forward() { run_diff("block_arg_forward"); }
#[test] fn do_while() { run_diff("do_while"); }
#[test] fn mixed_splat_call() { run_diff("mixed_splat_call"); }
#[test] fn protected_method() { run_diff("protected_method"); }
#[test] fn regex_sub() { run_diff("regex_sub"); }
#[test] fn method_object() { run_diff("method_object"); }
#[test] fn vararg_lambda() { run_diff("vararg_lambda"); }
#[test] fn method_to_proc() { run_diff("method_to_proc"); }
#[test] fn unbound_method() { run_diff("unbound_method"); }
#[test] fn method_introspect() { run_diff("method_introspect"); }
#[test] fn method_equality() { run_diff("method_equality"); }
#[test] fn method_compose() { run_diff("method_compose"); }
#[test] fn method_curry() { run_diff("method_curry"); }
#[test] fn class_instance_method() { run_diff("class_instance_method"); }
#[test] fn proc_curry_compose() { run_diff("proc_curry_compose"); }
#[test] fn method_to_proc_explicit() { run_diff("method_to_proc_explicit"); }
#[test] fn method_owner_receiver() { run_diff("method_owner_receiver"); }
#[test] fn integer_digits_bits() { run_diff("integer_digits_bits"); }
#[test] fn string_squeeze() { run_diff("string_squeeze"); }
#[test] fn string_scan() { run_diff("string_scan"); }
#[test] fn array_chunk_while() { run_diff("array_chunk_while"); }
#[test] fn min_max_by_n() { run_diff("min_max_by_n"); }
#[test] fn string_pad() { run_diff("string_pad"); }
#[test] fn array_bsearch() { run_diff("array_bsearch"); }
#[test] fn hash_transform() { run_diff("hash_transform"); }
#[test] fn block_param_and_given() { run_diff("block_param_and_given"); }
#[test] fn hash_slice_except() { run_diff("hash_slice_except"); }
#[test] fn array_take_drop_while() { run_diff("array_take_drop_while"); }
#[test] fn array_tally() { run_diff("array_tally"); }
#[test] fn comparable_clamp_range() { run_diff("comparable_clamp_range"); }
#[test] fn float_precision() { run_diff("float_precision"); }
#[test] fn compact_filter_map() { run_diff("compact_filter_map"); }
#[test] fn array_combinatorics() { run_diff("array_combinatorics"); }
#[test] fn array_assoc_rassoc() { run_diff("array_assoc_rassoc"); }
#[test] fn range_cover_step() { run_diff("range_cover_step"); }
#[test] fn object_reflection() { run_diff("object_reflection"); }
#[test] fn constant_path_write() { run_diff("constant_path_write"); }
#[test] fn method_hash_source() { run_diff("method_hash_source"); }
#[test] fn string_encoding_stubs() { run_diff("string_encoding_stubs"); }
#[test] fn pack_unpack() { run_diff("pack_unpack"); }
#[test] fn integer_literal_i64() { run_diff("integer_literal_i64"); }
#[test] fn lambda_slot_isolation() { run_diff("lambda_slot_isolation"); }
#[test] fn require_relative_main() { run_diff("require_relative_main"); }
