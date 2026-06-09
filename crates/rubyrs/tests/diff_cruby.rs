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

/// True iff the system `ruby` can `require '<gem_probe>'`. Lets a
/// gem-oracle diff skip gracefully (rather than fail) on a machine
/// where the blessed gem isn't installed — mirroring `ruby_available`.
#[cfg(feature = "stdlib")]
fn gem_available(gem_probe: &str) -> bool {
    Command::new("ruby")
        .arg("-e")
        .arg(format!("require '{gem_probe}'"))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Like `run_diff`, but the CRuby oracle runs with RubyGems ENABLED so
/// a blessed off-stdlib gem (e.g. ActiveSupport) is the parity oracle.
/// The same fixture `require`s the gem name on both runtimes: rubyrs
/// resolves it to the vendored pure-Ruby canon, CRuby to the real gem.
///
/// `gem_probe` is the `require` path used to detect availability; if
/// the host ruby lacks the gem the test skips (like a missing `ruby`),
/// so contributors without the gem installed aren't blocked. CI pins
/// and installs the gem, so the gate is live there.
#[cfg(feature = "stdlib")]
fn run_diff_gem(name: &str, gem_probe: &str) {
    if !ruby_available() {
        eprintln!("skipping diff_cruby::{} — `ruby` not on PATH", name);
        return;
    }
    if !gem_available(gem_probe) {
        eprintln!(
            "skipping diff_cruby::{} — `require '{}'` failed (gem not installed)",
            name, gem_probe
        );
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
    // No `--disable=gems`: the real gem must load on the oracle side.
    let theirs = Command::new("ruby")
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
#[test] fn hash_clear() { run_diff("hash_clear"); }
#[test] fn hash_key_clone() { run_diff("hash_key_clone"); }
#[test] fn hash_default_proc_set() { run_diff("hash_default_proc_set"); }
#[test] fn hash_subclass() { run_diff("hash_subclass"); }
#[test] fn hash_subclass_override() { run_diff("hash_subclass_override"); }
#[cfg(feature = "stdlib")]
#[test] fn set_merge() { run_diff("set_merge"); }
#[test] fn block_basics() { run_diff("block_basics"); }
#[test] fn class_basics() { run_diff("class_basics"); }
#[test] fn public_send() { run_diff("public_send"); }
#[test] fn def_class_method() { run_diff("def_class_method"); }
#[test] fn undef_keyword() { run_diff("undef_keyword"); }
#[test] fn condition_variable() { run_diff("condition_variable"); }
#[test] fn logger_basic() { run_diff("logger_basic"); }
#[test] fn for_loop() { run_diff("for_loop"); }
#[test] fn enumerable_find_ifnone() { run_diff("enumerable_find_ifnone"); }
#[test] fn backtick_sandbox() { run_diff("backtick_sandbox"); }
#[test] fn require_time_date() { run_diff("require_time_date"); }
#[test] fn yaml_is_psych() { run_diff("yaml_is_psych"); }
#[test] fn yaml_load() { run_diff("yaml_load"); }
#[test] fn yaml_load_file() { run_diff("yaml_load_file"); }
#[test] fn singleton_alias_method() { run_diff("singleton_alias_method"); }
#[test] fn singleton_class_body_more() { run_diff("singleton_class_body_more"); }
#[test] fn singleton_class_conditional_def() { run_diff("singleton_class_conditional_def"); }
#[test] fn singleton_class_bare_call() { run_diff("singleton_class_bare_call"); }
#[test] fn integer_size() { run_diff("integer_size"); }
#[test] fn class_public_methods() { run_diff("class_public_methods"); }
#[test] fn runtime_attr_accessor() { run_diff("runtime_attr_accessor"); }
#[test] fn file_join() { run_diff("file_join"); }
#[test] fn dir_glob() { run_diff("dir_glob"); }
#[test] fn symbol_basics() { run_diff("symbol_basics"); }
#[test] fn symbol_methods() { run_diff("symbol_methods"); }
#[test] fn symbol_inspect() { run_diff("symbol_inspect"); }
#[test] fn string_intern() { run_diff("string_intern"); }
#[test] fn private_class_method() { run_diff("private_class_method"); }
#[test] fn interpolation() { run_diff("interpolation"); }
#[test] fn interpolated_symbol() { run_diff("interpolated_symbol"); }
#[test] fn rescue_basics() { run_diff("rescue_basics"); }
#[test] fn fizzbuzz_15() { run_diff("fizzbuzz_15"); }
#[test] fn inheritance() { run_diff("inheritance"); }
#[test] fn custom_exception() { run_diff("custom_exception"); }
#[test] fn ensure_basics() { run_diff("ensure_basics"); }
#[test] fn control_flow() { run_diff("control_flow"); }
#[test] fn range_basics() { run_diff("range_basics"); }
#[test] fn range_to_s_inspect() { run_diff("range_to_s_inspect"); }
#[test] fn range_hash() { run_diff("range_hash"); }
#[test] fn method_inspect_format() { run_diff("method_inspect_format"); }
#[test] fn method_inspect_singleton() { run_diff("method_inspect_singleton"); }
#[test] fn method_inspect_params() { run_diff("method_inspect_params"); }
#[test] fn method_inspect_source_location() { run_diff("method_inspect_source_location"); }
#[test] fn array_hash_content_hash() { run_diff("array_hash_content_hash"); }
#[test] fn object_itself_tap_then() { run_diff("object_itself_tap_then"); }
#[test] fn instance_variable_defined() { run_diff("instance_variable_defined"); }
#[test] fn object_dup_clone() { run_diff("object_dup_clone"); }
#[test] fn object_methods_introspection() { run_diff("object_methods_introspection"); }
#[test] fn object_extend() { run_diff("object_extend"); }
#[test] fn object_define_singleton_method() { run_diff("object_define_singleton_method"); }
#[test] fn object_method_getters() { run_diff("object_method_getters"); }
#[test] fn define_method_2arg_form() { run_diff("define_method_2arg_form"); }
#[test] fn enumerable_filter() { run_diff("enumerable_filter"); }
#[test] fn enumerable_aggregate() { run_diff("enumerable_aggregate"); }
#[test] fn int_string_basics() { run_diff("int_string_basics"); }
#[test] fn array_extras() { run_diff("array_extras"); }
#[test] fn array_range_member() { run_diff("array_range_member"); }
#[test] fn default_superclass_object() { run_diff("default_superclass_object"); }
#[test] fn root_hierarchy() { run_diff("root_hierarchy"); }
#[test] fn kernel_builtin_reflection() { run_diff("kernel_builtin_reflection"); }
#[test] fn basic_object_builtin_reflection() { run_diff("basic_object_builtin_reflection"); }
#[test] fn universal_object_methods() { run_diff("universal_object_methods"); }
#[test] fn class_compare() { run_diff("class_compare"); }
#[test] fn sinatra_dsl_shape() { run_diff("sinatra_dsl_shape"); }
#[test] fn anon_block_forward() { run_diff("anon_block_forward"); }
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
#[test] fn integer_chr() { run_diff("integer_chr"); }
#[test] fn integer_chr_encoding() { run_diff("integer_chr_encoding"); }
#[test] fn string_new() { run_diff("string_new"); }
#[test] fn string_each_byte() { run_diff("string_each_byte"); }
#[test] fn integer_to_s_radix() { run_diff("integer_to_s_radix"); }
#[test] fn string_index_offset() { run_diff("string_index_offset"); }
#[test] fn hash_each_key_value() { run_diff("hash_each_key_value"); }
#[test] fn string_to_i_radix() { run_diff("string_to_i_radix"); }
#[test] fn kernel_integer_radix() { run_diff("kernel_integer_radix"); }
#[test] fn kernel_sprintf() { run_diff("kernel_sprintf"); }
#[test] fn module_new() { run_diff("module_new"); }
#[test] fn time_basics() { run_diff("time_basics"); }
#[test] fn time_strftime() { run_diff("time_strftime"); }
#[test] fn enumerable_by() { run_diff("enumerable_by"); }
#[test] fn super_call() { run_diff("super_call"); }
#[test] fn return_nonlocal() { run_diff("return_nonlocal"); }
#[test] fn methods_batch() { run_diff("methods_batch"); }
#[test] fn rescue_primitive() { run_diff("rescue_primitive"); }
#[test] fn zero_division() { run_diff("zero_division"); }
#[test] fn superinstr_binop_locals() { run_diff("superinstr_binop_locals"); }
#[test] fn multi_write() { run_diff("multi_write"); }
#[test] fn splat_multi_write() { run_diff("splat_multi_write"); }
#[test] fn string_format() { run_diff("string_format"); }
#[test] fn array_zip() { run_diff("array_zip"); }
#[test] fn comparable() { run_diff("comparable"); }
#[test] fn instance_variable_get_set() { run_diff("instance_variable_get_set"); }
#[test] fn inheritance_constant_path() { run_diff("inheritance_constant_path"); }
#[test] fn regex_freeze() { run_diff("regex_freeze"); }
#[test] fn class_allocate() { run_diff("class_allocate"); }
#[test] fn bare_class_methods() { run_diff("bare_class_methods"); }
#[test] fn class_self_const() { run_diff("class_self_const"); }
#[test] fn class_self_cvar() { run_diff("class_self_cvar"); }
#[test] fn class_self_if_modifier() { run_diff("class_self_if_modifier"); }
#[test] fn class_self_alias_builtin() { run_diff("class_self_alias_builtin"); }
#[test] fn class_self_visibility() { run_diff("class_self_visibility"); }
#[test] fn env_nested_lookup() { run_diff("env_nested_lookup"); }
#[test] fn module_define_method() { run_diff("module_define_method"); }
#[test] fn singleton_class_class_eval() { run_diff("singleton_class_class_eval"); }
#[test] fn proc_arity() { run_diff("proc_arity"); }
#[test] fn kernel_array_via_method() { run_diff("kernel_array_via_method"); }
#[test] fn array_dup_clone() { run_diff("array_dup_clone"); }
#[test] fn module_const_reflection() { run_diff("module_const_reflection"); }
#[test] fn include_const_resolution() { run_diff("include_const_resolution"); }
#[test] fn include_const_precedence() { run_diff("include_const_precedence"); }
#[test] fn include_const_prepend_super() { run_diff("include_const_prepend_super"); }
#[test] fn include_const_reflection_nameerror() { run_diff("include_const_reflection_nameerror"); }
#[test] fn string_sub_bang() { run_diff("string_sub_bang"); }
#[test] fn op_assign() { run_diff("op_assign"); }
#[test] fn range_enumerable() { run_diff("range_enumerable"); }
#[test] fn string_search() { run_diff("string_search"); }
#[test] fn visibility() { run_diff("visibility"); }
#[test] fn visibility_error_message() { run_diff("visibility_error_message"); }
#[test] fn require_caller_dir_isolation() { run_diff("require_caller_dir_isolation"); }
#[test] fn string_chomp() { run_diff("string_chomp"); }
#[test] fn multiwrite_global() { run_diff("multiwrite_global"); }
#[test] fn retry_keyword() { run_diff("retry_keyword"); }
#[test] fn module_attr_legacy() { run_diff("module_attr_legacy"); }
#[test] fn alias_method_inherited_primitive() { run_diff("alias_method_inherited_primitive"); }
#[test] fn module_function() { run_diff("module_function"); }
#[test] fn anonymous_rest_param() { run_diff("anonymous_rest_param"); }
#[test] fn class_inherited_hook() { run_diff("class_inherited_hook"); }
#[test] fn kernel_caller() { run_diff("kernel_caller"); }
#[test] fn kernel_dir() { run_diff("kernel_dir"); }
#[test] fn kernel_load() { run_diff("kernel_load"); }
#[test] fn array_map_bang() { run_diff("array_map_bang"); }
#[test] fn closure_in_iter_capture() { run_diff("closure_in_iter_capture"); }
#[test] fn callable_coerce() { run_diff("callable_coerce"); }
#[test] fn module_function_bare() { run_diff("module_function_bare"); }
#[test] fn array_subscript_slice() { run_diff("array_subscript_slice"); }
#[test] fn array_assign_slice() { run_diff("array_assign_slice"); }
#[test] fn primitive_argc_buffer() { run_diff("primitive_argc_buffer"); }
#[test] fn string_subscript_slice() { run_diff("string_subscript_slice"); }
#[test] fn string_assign_slice() { run_diff("string_assign_slice"); }
#[cfg(feature = "regex")]
#[test] fn regex_lookaround() { run_diff("regex_lookaround"); }
#[cfg(feature = "regex")]
#[test] fn string_split_regex() { run_diff("string_split_regex"); }
#[test] fn lifecycle_hook_super() { run_diff("lifecycle_hook_super"); }
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
#[test] fn regex_class_methods() { run_diff("regex_class_methods"); }
#[test] fn splat_block_forwarding() { run_diff("splat_block_forwarding"); }
#[test] fn safe_navigation() { run_diff("safe_navigation"); }
#[test] fn class_extend() { run_diff("class_extend"); }
#[test] fn super_splat_block() { run_diff("super_splat_block"); }
#[test] fn array_splat_coerce() { run_diff("array_splat_coerce"); }
#[test] fn super_in_block() { run_diff("super_in_block"); }
// `super` in a block yielded by ANOTHER object's method must resolve to
// the lexical-owner method's super-chain, not the intervening frame's.
#[test] fn super_block_foreign_yield() { run_diff("super_block_foreign_yield"); }
// `yield(a: 1)` keyword sugar — trailing KeywordHashNode yielded as a Hash.
#[test] fn yield_kwargs() { run_diff("yield_kwargs"); }
// Implicit block/lambda params: numbered `_1`/`_2` and Ruby 3.4 `it`.
#[test] fn numbered_and_it_params() { run_diff("numbered_and_it_params"); }
// Multiple at_exit handlers run LIFO — GC-rooting regression guard.
#[test] fn at_exit_many() { run_diff("at_exit_many"); }
// Anonymous `*`/`**`/`&` and `...` argument forwarding (empty kwrest drops).
#[test] fn arg_forwarding() { run_diff("arg_forwarding"); }
// `respond_to?` sees methods reopened onto / included into a core class.
#[test] fn respond_to_reopened() { run_diff("respond_to_reopened"); }
// respond_to? consults a user respond_to_missing? on resolution miss.
#[test] fn respond_to_missing() { run_diff("respond_to_missing"); }
// `def name` evaluates to :name (enables `private def …` modifier idiom).
#[test] fn def_returns_symbol() { run_diff("def_returns_symbol"); }
// Array/Hash/Range reach Enumerable methods with no native arm.
#[test] fn enumerable_fallback() { run_diff("enumerable_fallback"); }
// sprintf `0` flag zero-pads floats even with a precision.
#[test] fn sprintf_zero_pad_float() { run_diff("sprintf_zero_pad_float"); }
// sprintf `%e`/`%E` scientific + `%g`/`%G` general notation.
#[test] fn sprintf_scientific() { run_diff("sprintf_scientific"); }
// p calls user inspect; puts/print call user to_s.
#[test] fn p_puts_user_inspect() { run_diff("p_puts_user_inspect"); }
// define_method block with |**kw| binds kwargs.
#[test] fn define_method_kwrest() { run_diff("define_method_kwrest"); }
// Refinements: refine/using (Tier-1 global activation).
#[test] fn refinements() { run_diff("refinements"); }
// Array#flatten recurses fully; flatten(n) depth; flatten! in place.
#[test] fn array_flatten() { run_diff("array_flatten"); }
// `$!` is dynamically scoped: reverts after rescue/ensure body, on
// nested rescue, and on `return` out of a handler.
#[test] fn dollar_bang_scope() { run_diff("dollar_bang_scope"); }
// Exception#inspect via `p`/`pp` keeps the message (`#<Class: msg>`),
// empty message → bare class name; matches explicit `exc.inspect`.
#[test] fn exception_inspect() { run_diff("exception_inspect"); }
// Collection inspect dispatches per-element (custom/Exception) and is
// cycle-safe (`[...]`/`{...}` instead of a native stack overflow).
#[test] fn nested_inspect_cycle() { run_diff("nested_inspect_cycle"); }
// Float#to_s/#inspect: CRuby dtoa fixed-vs-scientific notation.
#[test] fn float_format() { run_diff("float_format"); }
// Float#floor(n)/#ceil(n) with ndigits + Float#divmod.
#[test] fn float_floor_ceil_divmod() { run_diff("float_floor_ceil_divmod"); }
// Array#cycle: block form (n/∞) + no-block Enumerator (first/take).
#[test] fn array_cycle() { run_diff("array_cycle"); }
// Array#rotate / #rotate(n) / #rotate! — left-rotate, wraps mod len.
#[test] fn array_rotate() { run_diff("array_rotate"); }
// String#lines / #each_line — split keeping the separator.
#[test] fn string_lines() { run_diff("string_lines"); }
// No-block Integer#times/#upto/#downto return an Enumerator.
#[test] fn integer_noblock_enum() { run_diff("integer_noblock_enum"); }
// Enumerable#each_slice/#each_cons (Enumerator, Integer iters, Range).
#[test] fn enumerator_each_slice_cons() { run_diff("enumerator_each_slice_cons"); }
// Numeric#step — positional + keyword (to:/by:), Integer + Float.
#[test] fn numeric_step() { run_diff("numeric_step"); }
// Symbol#to_proc — explicit `:sym.to_proc` conversion (literal &:sym is
// covered by symbol_to_proc).
#[test] fn symbol_to_proc_explicit() { run_diff("symbol_to_proc_explicit"); }
// String includes Comparable → #between? / #clamp.
#[test] fn string_comparable() { run_diff("string_comparable"); }
// Small gaps: NilClass#to_a/#to_h, String#getbyte.
#[test] fn nil_getbyte_gaps() { run_diff("nil_getbyte_gaps"); }
// Integer#gcdlcm → [gcd, lcm].
#[test] fn integer_gcdlcm() { run_diff("integer_gcdlcm"); }
// Range#step with Float bounds/step, inclusive + exclusive.
#[test] fn range_step_float() { run_diff("range_step_float"); }
// Hash#values_at / #each_key / #each_value (block + Enumerator).
#[test] fn hash_values_at_each_kv() { run_diff("hash_values_at_each_kv"); }
// Array.new no-block forms (size / size+fill / Array copy).
#[test] fn array_new_forms() { run_diff("array_new_forms"); }
// Array#transpose.
#[test] fn array_transpose() { run_diff("array_transpose"); }
// String#start_with?(Regexp, variadic) + #each_byte Enumerator.
#[cfg(feature = "regex")]
#[test] fn string_start_with_regex_each_byte() { run_diff("string_start_with_regex_each_byte"); }
// Boolean / NilClass logical methods & | ^.
#[test] fn bool_nil_logical_ops() { run_diff("bool_nil_logical_ops"); }
// String#partition/#rpartition (str+regex), #insert, #delete.
#[cfg(feature = "regex")]
#[test] fn string_partition_insert_delete() { run_diff("string_partition_insert_delete"); }
// Struct: keyword_init, block form, to_h/[]/each, inspect.
#[test] fn struct_features() { run_diff("struct_features"); }
// Ruby 3.2 Data.define — immutable value objects.
#[test] fn data_define() { run_diff("data_define"); }
// `Set[...]` constructor (stdlib-vendored Set surface).
#[cfg(feature = "stdlib")]
#[test] fn set_bracket_ctor() { run_diff("set_bracket_ctor"); }
// Pattern matching: case/in, `=> pat`, `in pat`, deconstruct protocol.
#[test] fn pattern_matching() { run_diff("pattern_matching"); }
// Find patterns `[*pre, mâ¦, *post]`.
#[test] fn find_pattern() { run_diff("find_pattern"); }
// Flip-flop `a..b` / `a...b` in boolean context.
#[test] fn flip_flop() { run_diff("flip_flop"); }
// `END { }` → at_exit, `BEGIN { }` → inline; interleaved LIFO with at_exit.
#[test] fn begin_end_blocks() { run_diff("begin_end_blocks"); }
#[test] fn hash_to_hash() { run_diff("hash_to_hash"); }
#[test] fn system_stack_error() { run_diff("system_stack_error"); }
#[test] fn method_missing_on_class() { run_diff("method_missing_on_class"); }
#[test] fn multi_write_index() { run_diff("multi_write_index"); }
#[test] fn match_data_full() { run_diff("match_data_full"); }
#[test] fn match_data_named() { run_diff("match_data_named"); }
#[test] fn class_new_and_dynamic_super() { run_diff("class_new_and_dynamic_super"); }
#[test] fn hash_pair_yield() { run_diff("hash_pair_yield"); }
#[test] fn array_predicates_no_block() { run_diff("array_predicates_no_block"); }
#[test] fn alias_method_runtime() { run_diff("alias_method_runtime"); }
#[test] fn exception_hierarchy() { run_diff("exception_hierarchy"); }
#[test] fn stack_depth_guard() { run_diff("stack_depth_guard"); }
#[test] fn exception_full_message() { run_diff("exception_full_message"); }
#[test] fn errno_extended() { run_diff("errno_extended"); }
#[test] fn bare_super_splat_forwarding() { run_diff("bare_super_splat_forwarding"); }
#[test] fn kwarg_computed_defaults() { run_diff("kwarg_computed_defaults"); }
#[test] fn exception_backtrace() { run_diff("exception_backtrace"); }
#[test] fn object_freeze() { run_diff("object_freeze"); }
#[test] fn super_kwarg_splat() { run_diff("super_kwarg_splat"); }
#[test] fn call_or_op_write() { run_diff("call_or_op_write"); }
#[test] fn multi_write_splat_call() { run_diff("multi_write_splat_call"); }
#[test] fn forwardable_shim() { run_diff("forwardable_shim"); }
#[test] fn delegate_shim() { run_diff("delegate_shim"); }
#[test] fn module_const_set() { run_diff("module_const_set"); }
#[test] fn class_new_no_block() { run_diff("class_new_no_block"); }
#[test] fn class_new_inherited_hook() { run_diff("class_new_inherited_hook"); }
#[test] fn require_ipaddr_stub() { run_diff("require_ipaddr_stub"); }
#[test] fn gem_version_shim() { run_diff("gem_version_shim"); }
#[test] fn array_product() { run_diff("array_product"); }
#[test] fn file_separator_consts() { run_diff("file_separator_consts"); }
#[test] fn regexp_union() { run_diff("regexp_union"); }
#[test] fn file_posix_flag_consts() { run_diff("file_posix_flag_consts"); }
#[test] fn file_open_read() { run_diff("file_open_read"); }
#[test] fn string_index_multibyte() { run_diff("string_index_multibyte"); }
#[test] fn autoload_scoped() { run_diff("autoload_scoped"); }
#[test] fn require_openssl_zlib_stub() { run_diff("require_openssl_zlib_stub"); }
#[test] fn super_to_primitive() { run_diff("super_to_primitive"); }
#[test] fn fancy_regex_captures() { run_diff("fancy_regex_captures"); }
#[test] fn numeric_comparable() { run_diff("numeric_comparable"); }
#[test] fn call_splat_coerce() { run_diff("call_splat_coerce"); }
#[test] fn call_kwsplat_empty() { run_diff("call_kwsplat_empty"); }
#[test] fn array_reduce_init_sym() { run_diff("array_reduce_init_sym"); }
#[test] fn regexp_options_carrier() { run_diff("regexp_options_carrier"); }
#[test] fn regex_flags() { run_diff("regex_flags"); }
#[cfg(feature = "regex")]
#[test] fn regex_charclass_octal() { run_diff("regex_charclass_octal"); }
#[test] fn forwardable_dotted_accessor() { run_diff("forwardable_dotted_accessor"); }
#[test] fn block_kwrest_param() { run_diff("block_kwrest_param"); }
#[test] fn struct_factory() { run_diff("struct_factory"); }
#[test] fn const_get_inheritance_walk() { run_diff("const_get_inheritance_walk"); }
#[test] fn anon_class_const_set() { run_diff("anon_class_const_set"); }
#[test] fn bare_object_method_call() { run_diff("bare_object_method_call"); }
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
#[test] fn last_match_globals() { run_diff("last_match_globals"); }
#[test] fn case_when_regex_globals() { run_diff("case_when_regex_globals"); }
#[test] fn interpolated_regex() { run_diff("interpolated_regex"); }
#[test] fn regex_g_anchor() { run_diff("regex_g_anchor"); }
#[test] fn string_unary_plus_minus() { run_diff("string_unary_plus_minus"); }
#[test] fn return_multi_value() { run_diff("return_multi_value"); }
#[test] fn private_self_receiver() { run_diff("private_self_receiver"); }
#[test] fn method_call_block() { run_diff("method_call_block"); }
#[test] fn yield_through_nested_block() { run_diff("yield_through_nested_block"); }
#[test] fn frame_locals_pool() { run_diff("frame_locals_pool"); }
#[test] fn fxhash_internal_maps() { run_diff("fxhash_internal_maps"); }
#[test] fn fxhash_vm_maps() { run_diff("fxhash_vm_maps"); }
#[test] fn inline_cache_dispatch() { run_diff("inline_cache_dispatch"); }
#[test] fn p0_each_yield_composability() { run_diff("p0_each_yield_composability"); }
#[test] fn string_dump() { run_diff("string_dump"); }
#[test] fn string_count() { run_diff("string_count"); }
#[test] fn class_singleton_class() { run_diff("class_singleton_class"); }
#[cfg(feature = "regex")]
#[test] fn string_bracket_regex() { run_diff("string_bracket_regex"); }
#[test] fn array_delete() { run_diff("array_delete"); }
#[test] fn thread_current() { run_diff("thread_current"); }
#[test] fn eval_basics() { run_diff("eval_basics"); }
#[test] fn instance_method_string() { run_diff("instance_method_string"); }
#[test] fn unbound_method_bind_call() { run_diff("unbound_method_bind_call"); }
#[test] fn method_snapshot_survives_remove() { run_diff("method_snapshot_survives_remove"); }
#[test] fn class_remove_method() { run_diff("class_remove_method"); }
#[cfg(feature = "bignum")]
#[test] fn bignum_phase_a() { run_diff("bignum_phase_a"); }
#[cfg(feature = "bignum")]
#[test] fn bignum_phase_b_pow() { run_diff("bignum_phase_b_pow"); }
#[cfg(feature = "bignum")]
#[test] fn bignum_phase_b_unary() { run_diff("bignum_phase_b_unary"); }
#[cfg(feature = "bignum")]
#[test] fn bignum_phase_b_pow_mod() { run_diff("bignum_phase_b_pow_mod"); }
#[cfg(feature = "bignum")]
#[test] fn bignum_phase_b_digits() { run_diff("bignum_phase_b_digits"); }
#[cfg(feature = "bignum")]
#[test] fn bignum_phase_b_to_s_sprintf() { run_diff("bignum_phase_b_to_s_sprintf"); }
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
#[test] fn method_name_getter() { run_diff("method_name_getter"); }
#[test] fn method_super_method() { run_diff("method_super_method"); }
#[test] fn method_dup_clone() { run_diff("method_dup_clone"); }
#[test] fn method_original_name() { run_diff("method_original_name"); }
#[test] fn module_included_prepended_hooks() { run_diff("module_included_prepended_hooks"); }
#[test] fn module_extended_hook() { run_diff("module_extended_hook"); }
#[test] fn module_method_lifecycle_hooks() { run_diff("module_method_lifecycle_hooks"); }
#[test] fn singleton_method_added_hook() { run_diff("singleton_method_added_hook"); }
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
#[test] fn array_first_last_n() { run_diff("array_first_last_n"); }
#[test] fn range_first_last_n() { run_diff("range_first_last_n"); }
#[test] fn nonlocal_return_from_block() { run_diff("nonlocal_return_from_block"); }
#[test] fn block_break_value() { run_diff("block_break_value"); }
#[test] fn block_break_value_final() { run_diff("block_break_value_final"); }
#[test] fn array_chunk_separator() { run_diff("array_chunk_separator"); }
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
#[test] fn pack_endian() { run_diff("pack_endian"); }
#[test] fn integer_bit_index() { run_diff("integer_bit_index"); }
#[test] fn class_instance_method_primitive() { run_diff("class_instance_method_primitive"); }
#[test] fn integer_literal_i64() { run_diff("integer_literal_i64"); }
#[test] fn lambda_slot_isolation() { run_diff("lambda_slot_isolation"); }
#[test] fn require_relative_main() { run_diff("require_relative_main"); }
#[test] fn break_in_while() { run_diff("break_in_while"); }
#[test] fn next_in_while() { run_diff("next_in_while"); }
#[test] fn break_next_ensure() { run_diff("break_next_ensure"); }
#[test] fn singleton_class_body() { run_diff("singleton_class_body"); }
#[test] fn global_variables() { run_diff("global_variables"); }
#[test] fn op_assign_extended() { run_diff("op_assign_extended"); }
#[test] fn mutex_stub() { run_diff("mutex_stub"); }
#[test] fn kernel_instance_method() { run_diff("kernel_instance_method"); }
#[test] fn constant_scoping() { run_diff("constant_scoping"); }
#[test] fn cext_msgpack_bigint() { run_diff("cext_msgpack_bigint"); }
#[test] fn string_unpack1() { run_diff("string_unpack1"); }
#[test] fn string_encoding_query() { run_diff("string_encoding_query"); }
#[test] fn string_inspect_control() { run_diff("string_inspect_control"); }
#[test] fn pack_directives_extra() { run_diff("pack_directives_extra"); }
#[test] fn string_high_byte_literal() { run_diff("string_high_byte_literal"); }
#[test] fn singleton_class_attr() { run_diff("singleton_class_attr"); }
#[test] fn alias_keyword() { run_diff("alias_keyword"); }
#[test] fn kernel_array_coerce() { run_diff("kernel_array_coerce"); }
#[test] fn kernel_p_multi_arg() { run_diff("kernel_p_multi_arg"); }
#[test] fn block_local_freshness() { run_diff("block_local_freshness"); }
#[test] fn cext_msgpack_timestamp() { run_diff("cext_msgpack_timestamp"); }
#[test] fn class_method_defined() { run_diff("class_method_defined"); }
#[test] fn rational_literal() { run_diff("rational_literal"); }
#[test] fn cext_msgpack_pure_ruby_load() { run_diff("cext_msgpack_pure_ruby_load"); }
#[test] fn alias_singleton_keyword() { run_diff("alias_singleton_keyword"); }
#[test] fn class_qualified_name() { run_diff("class_qualified_name"); }
#[test] fn alias_method_primitive() { run_diff("alias_method_primitive"); }
#[test] fn super_splat() { run_diff("super_splat"); }
#[test] fn object_send() { run_diff("object_send"); }
#[test] fn class_variables() { run_diff("class_variables"); }
#[test] fn cvar_hierarchy() { run_diff("cvar_hierarchy"); }
#[test] fn load_path() { run_diff("load_path"); }
#[test] fn source_location() { run_diff("source_location"); }
#[test] fn source_line() { run_diff("source_line"); }
#[test] fn require_xpkg() { run_diff("require_xpkg"); }
#[test] fn stdlib_require_stub() { run_diff("stdlib_require_stub"); }
#[test] fn module_vs_class() { run_diff("module_vs_class"); }
#[test] fn module_introspection() { run_diff("module_introspection"); }
#[test] fn instance_methods_visibility() { run_diff("instance_methods_visibility"); }
#[test] fn module_include_typecheck() { run_diff("module_include_typecheck"); }
#[test] fn module_constants_included() { run_diff("module_constants_included"); }
#[test] fn class_level_ivars() { run_diff("class_level_ivars"); }
#[test] fn enumerable_stub() { run_diff("enumerable_stub"); }
#[test] fn object_and_string_hash() { run_diff("object_and_string_hash"); }
#[test] fn module_prepend() { run_diff("module_prepend"); }
#[test] fn block_arg_nil() { run_diff("block_arg_nil"); }
#[test] fn rescue_multi_class() { run_diff("rescue_multi_class"); }
#[test] fn rescue_constant_path() { run_diff("rescue_constant_path"); }
#[test] fn rescue_nested_constant() { run_diff("rescue_nested_constant"); }
#[test] fn throw_catch() { run_diff("throw_catch"); }
#[test] fn string_split_limit() { run_diff("string_split_limit"); }
#[test] fn string_split_awk() { run_diff("string_split_awk"); }
#[test] fn m27_hash_to_s() { run_diff("m27_hash_to_s"); }
#[test] fn m27_middle_splat() { run_diff("m27_middle_splat"); }
#[test] fn m27_define_method_blockarg() { run_diff("m27_define_method_blockarg"); }
#[test] fn class_qualified_separates() { run_diff("class_qualified_separates"); }
#[test] fn class_cref_walk() { run_diff("class_cref_walk"); }
#[test] fn module_nesting() { run_diff("module_nesting"); }
#[test] fn defined_constant_path() { run_diff("defined_constant_path"); }
#[test] fn random_class() { run_diff("random_class"); }
#[test] fn securerandom_seeded() { run_diff("securerandom_seeded"); }
#[cfg(feature = "stdlib")]
#[test] fn stdlib_pathname() { run_diff("stdlib_pathname"); }
#[cfg(feature = "stdlib")]
#[test] fn stdlib_set() { run_diff("stdlib_set"); }
#[cfg(feature = "stdlib")]
#[test] fn stdlib_stringio() { run_diff("stdlib_stringio"); }
#[test] fn uninitialized_constant() { run_diff("uninitialized_constant"); }
#[test] fn singleton_class_prepend() { run_diff("singleton_class_prepend"); }
#[test] fn tilt_load_capabilities() { run_diff("tilt_load_capabilities"); }
#[test] fn hash_new_default_block() { run_diff("hash_new_default_block"); }
#[test] fn array_sort_block() { run_diff("array_sort_block"); }
#[test] fn class_new_override() { run_diff("class_new_override"); }
#[test] fn backreference_globals() { run_diff("backreference_globals"); }
#[test] fn class_path_nested() { run_diff("class_path_nested"); }
#[test] fn encoding_stub() { run_diff("encoding_stub"); }
#[cfg(feature = "stdlib")]
#[test] fn stdlib_strscan() { run_diff("stdlib_strscan"); }
#[cfg(feature = "stdlib")]
#[test] fn json_roundtrip() { run_diff("json_roundtrip"); }
// ActiveSupport-lite core-ext (ADR 0026 menu item 3). Oracle is the
// real `activesupport` gem (RubyGems enabled) — pinned + installed in
// CI; skips locally when the gem isn't present.
#[cfg(feature = "stdlib")]
#[test] fn activesupport_core_ext() { run_diff_gem("activesupport_core_ext", "active_support/all"); }
#[cfg(feature = "stdlib")]
#[test] fn activesupport_duration() { run_diff_gem("activesupport_duration", "active_support/all"); }
#[test] fn fixed_arity_fast_path() { run_diff("fixed_arity_fast_path"); }
#[test] fn reopen_primitive_bare_call() { run_diff("reopen_primitive_bare_call"); }
#[test] fn gsub_block_captures() { run_diff("gsub_block_captures"); }
#[test] fn gsub_block_binary_bytes() { run_diff("gsub_block_binary_bytes"); }
#[test] fn match_data_inspect() { run_diff("match_data_inspect"); }
#[test] fn array_new_block_form() { run_diff("array_new_block_form"); }
#[test] fn hash_indexed() { run_diff("hash_indexed"); }
#[test] fn hash_sizes() { run_diff("hash_sizes"); }
#[test] fn struct_anon_ivars() { run_diff("struct_anon_ivars"); }
#[test] fn struct_in_container_gc() { run_diff("struct_in_container_gc"); }
#[test] fn time_parse() { run_diff("time_parse"); }
#[test] fn file_open_write() { run_diff("file_open_write"); }
#[test] fn file_gets_separator() { run_diff("file_gets_separator"); }
#[test] fn file_write_handle_read() { run_diff("file_write_handle_read"); }
#[test] fn file_write_bytecount() { run_diff("file_write_bytecount"); }
#[test] fn enumerable_module() { run_diff("enumerable_module"); }
#[test] fn super_block_literal() { run_diff("super_block_literal"); }
#[cfg(feature = "stdlib")]
#[test] fn pathname_ascend() { run_diff("pathname_ascend"); }
#[cfg(feature = "stdlib")]
#[test] fn pathname_plus() { run_diff("pathname_plus"); }
#[cfg(feature = "stdlib")]
#[test] fn set_enumerable() { run_diff("set_enumerable"); }
#[cfg(feature = "stdlib")]
#[test] fn set_ops() { run_diff("set_ops"); }
#[cfg(feature = "stdlib")]
#[test] fn stringscanner_full() { run_diff("stringscanner_full"); }
#[cfg(feature = "stdlib")]
#[test] fn stringscanner_units() { run_diff("stringscanner_units"); }
#[cfg(feature = "stdlib")]
#[test] fn yaml_psych_error() { run_diff("yaml_psych_error"); }
#[cfg(feature = "stdlib")]
#[test] fn digest_hexdigest() { run_diff("digest_hexdigest"); }
#[test] fn regex_line_anchors() { run_diff("regex_line_anchors"); }
#[test] fn array_insert() { run_diff("array_insert"); }
#[test] fn array_insert_too_big() { run_diff("array_insert_too_big"); }
#[test] fn array_block_mutation() { run_diff("array_block_mutation"); }
#[test] fn array_filter_break() { run_diff("array_filter_break"); }
#[test] fn sort_class_spaceship() { run_diff("sort_class_spaceship"); }
#[test] fn string_delete_affix() { run_diff("string_delete_affix"); }
#[test] fn string_index_substr() { run_diff("string_index_substr"); }
#[test] fn bare_is_a() { run_diff("bare_is_a"); }
#[test] fn massign_coerce() { run_diff("massign_coerce"); }
#[test] fn regex_named_captures() { run_diff("regex_named_captures"); }
#[test] fn regex_fancy_gsub() { run_diff("regex_fancy_gsub"); }
#[test] fn regex_backref_replace() { run_diff("regex_backref_replace"); }
#[test] fn match_data_index() { run_diff("match_data_index"); }
#[test] fn last_match_named() { run_diff("last_match_named"); }
#[test] fn regex_match_char_offset() { run_diff("regex_match_char_offset"); }
#[test] fn file_fnmatch() { run_diff("file_fnmatch"); }
#[test] fn fileops_write() { run_diff("fileops_write"); }
#[test] fn errno_rescue() { run_diff("errno_rescue"); }
#[test] fn fileutils_cp_array() { run_diff("fileutils_cp_array"); }
#[test] fn fileutils_cp_r_mv() { run_diff("fileutils_cp_r_mv"); }
#[test] fn file_write_mode() { run_diff("file_write_mode"); }
#[test] fn file_fnmatch_globstar() { run_diff("file_fnmatch_globstar"); }
#[test] fn scoped_autoload() { run_diff("scoped_autoload"); }
#[test] fn explicit_recv_block_fastpath() { run_diff("explicit_recv_block_fastpath"); }
#[test] fn block_locals_pool_reuse() { run_diff("block_locals_pool_reuse"); }
#[test] fn anon_class_named_const_set() { run_diff("anon_class_named_const_set"); }
#[test] fn kernel_module_function_call() { run_diff("kernel_module_function_call"); }
#[test] fn namespaced_builtin_class_redef() { run_diff("namespaced_builtin_class_redef"); }
#[test] fn kwargs_brace_hash_positional() { run_diff("kwargs_brace_hash_positional"); }
#[cfg(feature = "stdlib")]
#[test] fn pathname_glob_relpath() { run_diff("pathname_glob_relpath"); }
#[cfg(feature = "stdlib")]
#[test] fn file_join_to_path() { run_diff("file_join_to_path"); }
#[test] fn alias_primitive_snapshot() { run_diff("alias_primitive_snapshot"); }
#[test] fn include_const_alias_nested() { run_diff("include_const_alias_nested"); }
#[test] fn enum_for_basic() { run_diff("enum_for_basic"); }
#[test] fn enumerator_new_yielder() { run_diff("enumerator_new_yielder"); }
#[test] fn yield_splat() { run_diff("yield_splat"); }
#[test] fn enum_multivalue() { run_diff("enum_multivalue"); }
#[test] fn enum_noblock() { run_diff("enum_noblock"); }
#[test] fn transform_noblock() { run_diff("transform_noblock"); }
#[test] fn enum_argforms() { run_diff("enum_argforms"); }
#[test] fn enum_next_peek() { run_diff("enum_next_peek"); }
#[test] fn range_noblock() { run_diff("range_noblock"); }
#[test] fn enum_size() { run_diff("enum_size"); }
#[test] fn each_slice_enum() { run_diff("each_slice_enum"); }
#[test] fn slice_when() { run_diff("slice_when"); }
#[test] fn float_constants() { run_diff("float_constants"); }
#[test] fn endless_range() { run_diff("endless_range"); }
#[test] fn enumerator_lazy() { run_diff("enumerator_lazy"); }
#[test] fn class_name_override() { run_diff("class_name_override"); }
#[test] fn cvar_lexical_block() { run_diff("cvar_lexical_block"); }
#[test] fn enumerable_mixin() { run_diff("enumerable_mixin"); }
#[test] fn block_given_lexical() { run_diff("block_given_lexical"); }
#[cfg(feature = "regex")]
#[test] fn scan_fancy_regex() { run_diff("scan_fancy_regex"); }
#[test] fn kernel_norecv_method() { run_diff("kernel_norecv_method"); }
// BigDecimal's parity oracle needs CRuby's real bigdecimal (a bundled
// gem `ruby --disable=gems` can't load), so use the gem-enabled oracle.
#[cfg(feature = "stdlib")]
#[test] fn bigdecimal_basic() { run_diff_gem("bigdecimal_basic", "bigdecimal"); }
#[test] fn gsub_hash() { run_diff("gsub_hash"); }
#[test] fn hash_delete_block() { run_diff("hash_delete_block"); }
#[test] fn string_casecmp() { run_diff("string_casecmp"); }
#[test] fn string_each_char() { run_diff("string_each_char"); }
// Set's richer surface lives in the stdlib-gated `stdlib_vendor/set.rb`
// (the default build ships only a minimal Set), so gate this fixture the
// same way as set_merge / set_enumerable above.
#[cfg(feature = "stdlib")]
#[test] fn set_collect_bang() { run_diff("set_collect_bang"); }
#[test] fn to_h() { run_diff("to_h"); }
#[test] fn thread_current_locals() { run_diff("thread_current_locals"); }
#[test] fn dynamic_base_const() { run_diff("dynamic_base_const"); }
// `$~` is frame-local: a callee's internal regex match must not leak
// into the caller's $1.. (uses =~, so regex-gated like scan_fancy_regex).
#[cfg(feature = "regex")]
#[test] fn frame_local_match() { run_diff("frame_local_match"); }
