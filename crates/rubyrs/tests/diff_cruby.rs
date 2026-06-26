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

    // TZ pinned to UTC on BOTH sides: rubyrs Time is Tier-1 UTC-only
    // (with a local/utc FLAVOUR bit matching TZ=UTC CRuby), so a
    // host-local CRuby would render different zone suffixes and make
    // time fixtures host-dependent.
    let ours = Command::new(rubyrs_bin())
        .current_dir(manifest_dir())
        .env("TZ", "UTC")
        .arg(&rb_rel)
        .output()
        .expect("failed to spawn rubyrs");
    let theirs = Command::new("ruby")
        .arg("--disable=gems")
        .current_dir(manifest_dir())
        .env("TZ", "UTC")
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
#[test] fn array_join_binary() { run_diff("array_join_binary"); }
#[test] fn array_push_variadic() { run_diff("array_push_variadic"); }
#[test] fn hash_basics() { run_diff("hash_basics"); }
#[test] fn hash_compare_by_identity() { run_diff("hash_compare_by_identity"); }
#[test] fn index_fast_path() { run_diff("index_fast_path"); }
#[test] fn hash_key_fast_path() { run_diff("hash_key_fast_path"); }
#[cfg(feature = "stdlib")]
#[test] fn os_surface_batch() { run_diff("os_surface_batch"); }
#[test] fn object_dup_regex_sym_hash_default() { run_diff("object_dup_regex_sym_hash_default"); }
#[test] fn stdio_argv_surface() { run_diff("stdio_argv_surface"); }
#[test] fn loaded_features() { run_diff("loaded_features"); }
#[test] fn caller_locations() { run_diff("caller_locations"); }
#[test] fn class_object_instance_dispatch() { run_diff("class_object_instance_dispatch"); }
#[test] fn metaclass_alias_undef() { run_diff("metaclass_alias_undef"); }
#[test] fn define_method_lexical_yield() { run_diff("define_method_lexical_yield"); }
#[test] fn encoding_defaults() { run_diff("encoding_defaults"); }
#[test] fn define_method_runtime_name() { run_diff("define_method_runtime_name"); }
#[test] fn undef_listing_and_sym_blockpass() { run_diff("undef_listing_and_sym_blockpass"); }
#[cfg(feature = "stdlib")]
#[test] fn remove_const_forwardable_proc() { run_diff("remove_const_forwardable_proc"); }
#[test] fn method_over_builtin() { run_diff("method_over_builtin"); }
#[test] fn remove_const_pending_autoload() { run_diff("remove_const_pending_autoload"); }
#[test] fn const_source_location() { run_diff("const_source_location"); }
#[cfg(feature = "stdlib")]
#[test] fn minitest_substrate_extras() { run_diff("minitest_substrate_extras"); }
#[test] fn string_to_s_passthrough() { run_diff("string_to_s_passthrough"); }
#[test] fn assign_expr_value() { run_diff("assign_expr_value"); }
#[test] fn begin_dollar_bang_snapshot_gc() { run_diff("begin_dollar_bang_snapshot_gc"); }
#[test] fn primitive_reopen_precedence() { run_diff("primitive_reopen_precedence"); }
#[test] fn dollar_tilde_scoping() { run_diff("dollar_tilde_scoping"); }
#[test] fn hash_clear() { run_diff("hash_clear"); }
#[test] fn hash_key_clone() { run_diff("hash_key_clone"); }
#[test] fn hash_default_proc_set() { run_diff("hash_default_proc_set"); }
#[test] fn hash_subclass() { run_diff("hash_subclass"); }
#[test] fn hash_subclass_override() { run_diff("hash_subclass_override"); }
// merge (block + non-block) preserves the receiver's subclass tag.
#[test] fn hash_subclass_merge() { run_diff("hash_subclass_merge"); }
#[test] fn hash_subclass_super_overrides() { run_diff("hash_subclass_super_overrides"); }
// A Hash/Array subclass that redefines `self.[]` reaches its own
// class method (not the native Hash[]/Array[] constructor); a plain
// subclass still gets the native tagged build. rack Rack::Headers[...].
#[test] fn subclass_class_aref() { run_diff("subclass_class_aref"); }
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
#[test] fn singleton_class_real_body() { run_diff("singleton_class_real_body"); }
#[test] fn singleton_alias_kernel_builtin() { run_diff("singleton_alias_kernel_builtin"); }
#[test] fn singleton_class_expr_value() { run_diff("singleton_class_expr_value"); }
#[test] fn integer_size() { run_diff("integer_size"); }
#[test] fn class_public_methods() { run_diff("class_public_methods"); }
#[test] fn runtime_attr_accessor() { run_diff("runtime_attr_accessor"); }
#[test] fn file_join() { run_diff("file_join"); }
#[test] fn file_basename_suffix() { run_diff("file_basename_suffix"); }
#[test] fn file_split() { run_diff("file_split"); }
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
#[test] fn bare_inspect_to_s() { run_diff("bare_inspect_to_s"); }
#[test] fn class_self_const() { run_diff("class_self_const"); }
#[test] fn class_self_cvar() { run_diff("class_self_cvar"); }
#[test] fn class_self_if_modifier() { run_diff("class_self_if_modifier"); }
#[test] fn class_self_alias_builtin() { run_diff("class_self_alias_builtin"); }
// `alias new! new` snapshots the builtin Class#new (no recursion when
// `new` is then redefined to call `new!`) — Sinatra's middleware wrap.
#[test] fn alias_builtin_new() { run_diff("alias_builtin_new"); }
// Bare `new(...) { block }` (implicit self = class) forwards its block
// to #initialize, including through bare `super(...) { block }` — the
// path previously discarded the block (Sinatra `provides:`/mustermann).
#[test] fn bare_new_block_forward() { run_diff("bare_new_block_forward"); }
// String#gsub!/sub! with a Hash replacement table (uri's
// _encode_uri_component → rack set_cookie_header).
#[test] fn gsub_bang_hash() { run_diff("gsub_bang_hash"); }
// String#slice! char-range on BINARY / non-UTF-8 bytes indexes by byte
// (rack-session decrypt's data.slice!(-32..-1) on a decoded cookie).
#[test] fn slice_bang_binary() { run_diff("slice_bang_binary"); }
// Proc#binding — a Binding over the block's scope (self + closed-over
// locals); unblocks erubi's `eval(engine.src, block.binding)` harness
// (full erubi spec: 100 runs, 0 failures).
#[test] fn proc_binding() { run_diff("proc_binding"); }
// raise <non-exception> → TypeError "exception class/object expected".
#[test] fn raise_non_exception() { run_diff("raise_non_exception"); }
// bare super with named kwargs + **kwrest forwards them as keywords.
#[test] fn super_kwargs_kwrest() { run_diff("super_kwargs_kwrest"); }
#[test] fn super_forward_rest_kwargs() { run_diff("super_forward_rest_kwargs"); }
// trailing `k: v` inside an array literal is a Hash element.
#[test] fn array_trailing_kwhash() { run_diff("array_trailing_kwhash"); }
// Module#included_modules — modules in the ancestor chain.
#[test] fn included_modules() { run_diff("included_modules"); }
// Regexp#named_captures → name => ALL 1-based indices (duplicate
// (?<a>…) names), source order (mustermann splat collection).
#[cfg(feature = "regex")]
#[test] fn regexp_named_captures_dup() { run_diff("regexp_named_captures_dup"); }
// String#succ!/next! (Tilt compiled-method-name generation).
#[test] fn string_succ_bang() { run_diff("string_succ_bang"); }
// File.rename(old, new) — atomic rename.
#[test] fn file_rename() { run_diff("file_rename"); }
// foo(**empty, &blk) drops the empty kwsplat even with a block-pass
// (Tilt fixed-locals compiled_method.bind_call(scope, **locals, &block)).
#[test] fn kwsplat_empty_block() { run_diff("kwsplat_empty_block"); }
// NoMethodError < NameError < StandardError (CRuby exception hierarchy).
#[test] fn nomethoderror_is_nameerror() { run_diff("nomethoderror_is_nameerror"); }
// CRuby honours `# frozen-string-literal:` (hyphen form) too — Tilt
// emits it into compiled template source.
#[test] fn frozen_string_literal_hyphen() { run_diff("frozen_string_literal_hyphen"); }
// eval/class_eval line-offset arg maps the source's first line into the
// caller's coordinate system for backtraces (Tilt template line nums).
#[test] fn eval_line_offset() { run_diff("eval_line_offset"); }
// define_singleton_method(name, callable) on a heap primitive (Array/
// String/Hash) installs onto its per-instance eigenclass.
#[test] fn define_singleton_method_heap() { run_diff("define_singleton_method_heap"); }
// a singleton method on a heap primitive dispatches in block-call form.
#[test] fn heap_singleton_block_call() { run_diff("heap_singleton_block_call"); }
// Fiber.current — stable non-nil per-fiber Hash key (logger level_key).
#[test] fn fiber_current() { run_diff("fiber_current"); }
// Anonymous Struct class kept alive via its instances across the GC
// (its @__struct_attrs members Array must not be swept mid-construct).
#[test] fn struct_anon_gc() { run_diff("struct_anon_gc"); }
// Time.parse scans for an embedded timestamp (CRuby leniency).
#[test] fn time_parse_lenient() { run_diff("time_parse_lenient"); }
// `module ::Foo` / `class ::Bar` inside a class defines at top level.
#[test] fn absolute_module_def() { run_diff("absolute_module_def"); }
// super(*a, &b) with no superclass method falls to method_missing;
// super FROM method_missing raises (no recursion).
#[test] fn super_to_method_missing() { run_diff("super_to_method_missing"); }
// Pure-Ruby IPAddr (Tier 3 vendored): IPv4/IPv6 + CIDR + include?/===
// (rack-protection HostAuthorization). Needs the vendored stdlib source.
#[cfg(feature = "stdlib")]
#[test] fn ipaddr_basic() { run_diff("ipaddr_basic"); }
// Pure-Ruby Date/DateTime (Tier 3 vendored): JDN civil-date core,
// strftime/parse/arithmetic; now/today via Time.now.
#[cfg(feature = "stdlib")]
#[test] fn date_basic() { run_diff("date_basic"); }
// Module#autoload? returns nil once the constant is actually defined
// (Tilt's FinalizedMapping lazy lookups).
#[test] fn autoload_defined_nil() { run_diff("autoload_defined_nil"); }
// class<<self prepend surfaces in singleton_class.ancestors so
// remove_method on it restores dispatch (Tilt finalize! teardown).
#[test] fn singleton_class_prepend_ancestors() { run_diff("singleton_class_prepend_ancestors"); }
// `begin … rescue … else E … ensure … end` — the else body runs only
// on the no-exception path, its value is the begin's value, and an
// exception in else escapes the rescue chain (Sinatra/Tilt lazy_load).
#[test] fn begin_rescue_else() { run_diff("begin_rescue_else"); }
#[test] fn class_self_visibility() { run_diff("class_self_visibility"); }
#[test] fn env_nested_lookup() { run_diff("env_nested_lookup"); }
#[test] fn time_local_flavour() { run_diff("time_local_flavour"); }
#[test] fn proc_source_location() { run_diff("proc_source_location"); }
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
#[cfg(feature = "regex")]
#[test] fn regex_ascii_shorthand_classes() { run_diff("regex_ascii_shorthand_classes"); }
#[cfg(feature = "regex")]
#[test] fn regex_posix_unicode_classes() { run_diff("regex_posix_unicode_classes"); }
#[cfg(feature = "regex")]
#[test] fn file_read_bom_utf8() { run_diff("file_read_bom_utf8"); }
#[test] fn file_delete() { run_diff("file_delete"); }
#[test] fn array_subclass() { run_diff("array_subclass"); }
// Array#freeze enforcement: every mutator (incl. []=, block forms,
// no-op bang) raises FrozenError; clone preserves frozen, dup resets.
// rack Lock relies on `[].freeze.pop` raising so `ensure` unlocks.
#[test] fn array_freeze() { run_diff("array_freeze"); }
#[test] fn string_encoding_e1() { run_diff("string_encoding_e1"); }
#[test] fn string_encoding_compat() { run_diff("string_encoding_compat"); }
#[cfg(feature = "_encoding_full")]
#[test] fn encoding_iso_2022_jp() { run_diff("encoding_iso_2022_jp"); }
#[cfg(feature = "_encoding_full")]
#[test] fn encoding_full_latin1() { run_diff("encoding_full_latin1"); }
#[cfg(feature = "_encoding_full")]
#[test] fn encoding_full_seven() { run_diff("encoding_full_seven"); }
// E2 v3: reflection trio, Other-tag case ops, pivot-chain pairs
// + E3 registry-tag / ext:int transcoding reads.
#[cfg(feature = "_encoding_full")]
#[test] fn encoding_full_v3() { run_diff("encoding_full_v3"); }
// UTF-16LE/BE + BOM-form UTF-16: hand-rolled transcoder
// (encode/decode/round-trip, astral surrogate pairs, length/
// valid_encoding?, InvalidByteSequenceError on malformed bytes).
#[cfg(feature = "_encoding_full")]
#[test] fn encoding_full_utf16() { run_diff("encoding_full_utf16"); }
// UTF-32LE/BE + BOM-form UTF-32: same hand-rolled transcoder family.
#[cfg(feature = "_encoding_full")]
#[test] fn encoding_full_utf32() { run_diff("encoding_full_utf32"); }
// Strict Shift_JIS as a distinct Encoding from Windows-31J (shares the
// WHATWG transcoder; common plane round-trips). Tilt template encoding.
#[cfg(feature = "_encoding_full")]
#[test] fn encoding_shift_jis() { run_diff("encoding_shift_jis"); }
// eval/class_eval of non-UTF-8 source re-tags (+ transcodes) the
// produced string literals to the source encoding (Tilt templates).
#[cfg(feature = "_encoding_full")]
#[test] fn eval_source_encoding() { run_diff("eval_source_encoding"); }
// E3 core surface: File.read encoding: tags, default_external.
#[test] fn file_read_encoding() { run_diff("file_read_encoding"); }
#[test] fn lifecycle_hook_super() { run_diff("lifecycle_hook_super"); }
#[test] fn raise_two_arg() { run_diff("raise_two_arg"); }
// Kernel#Array(obj) coerces via to_ary→to_a before [obj]; backs
// [*obj] / `a, b = *obj` splat. rack `status, h, body = *response`.
#[test] fn array_coerce_splat() { run_diff("array_coerce_splat"); }
// `raise Class, msg, backtrace` — the explicit 3rd arg is stamped on
// the exception (`e.backtrace` returns it, incl. `[]`); 2-arg form
// keeps the call-site backtrace. rack ShowExceptions "unknown
// location" + QueryParser re-raise-with-backtrace.
#[test] fn raise_explicit_backtrace() { run_diff("raise_explicit_backtrace"); }
// `# frozen_string_literal: true` freezes plain string literals
// (interp stays mutable; eval has its own setting). rack
// Builder.parse_file frozen.ru rackup asserts `'frozen'.frozen?`.
#[test] fn frozen_string_literal() { run_diff("frozen_string_literal"); }
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
// `Symbol#match` / `#match?` delegate to `to_s` (ostruct/oj guard names
// with `name.match(/.../)`).
#[test] fn symbol_match() { run_diff("symbol_match"); }
// `Symbol#[]` delegates to `to_s[...]` (ostruct's method_missing peels a
// `name=` setter with `mid[/.*(?==\z)/m]`).
#[test] fn symbol_index() { run_diff("symbol_index"); }
#[test] fn integer_div() { run_diff("integer_div"); }
#[test] fn set_temporary_name() { run_diff("set_temporary_name"); }
#[test] fn dir_glob_block() { run_diff("dir_glob_block"); }
#[test] fn undef_object_private() { run_diff("undef_object_private"); }
#[test] fn super_to_builtin() { run_diff("super_to_builtin"); }
#[test] fn bare_extend() { run_diff("bare_extend"); }
#[test] fn is_a_extend() { run_diff("is_a_extend"); }
// `ERB::Util` h/html_escape + u/url_encode (rspec-core's HTML formatter
// does `include ERB::Util`). Vendored erb is stdlib-gated.
#[cfg(feature = "stdlib")]
#[test] fn erb_util() { run_diff("erb_util"); }

// `require "pp"` installs Object#pretty_inspect + PP (vendored under
// the stdlib feature). Surfaced by faraday's logging formatter.
#[cfg(feature = "stdlib")]
#[test] fn pp_pretty_inspect() { run_diff("pp_pretty_inspect"); }

// StringScanner#rest? — inverse of eos?. Surfaced by tzinfo's POSIX TZ
// parser (`while scanner.rest?`).
#[cfg(feature = "stdlib")]
#[test] fn strscan_rest_predicate() { run_diff("strscan_rest_predicate"); }
// pack/unpack `U` (UTF-8 codepoints) + `x` (skip/pad) — builder's
// `pack('U')`, tzinfo's `unpack('… x15 …')`.
#[test] fn pack_unpack_u_x() { run_diff("pack_unpack_u_x"); }
// `Set` is an autoloaded core class since Ruby 3.2 — usable without
// `require "set"` (multi_json's `Set.new` at load). stdlib-gated.
#[cfg(feature = "stdlib")]
#[test] fn set_autoload() { run_diff("set_autoload"); }
// alias_method / alias of a universal builtin (dup/hash/class/…) —
// literal + runtime forms (ostruct's `alias_method "#{m}!", m` loop).
#[test] fn alias_method_builtin() { run_diff("alias_method_builtin"); }
#[test] fn case_when() { run_diff("case_when"); }
#[test] fn modules() { run_diff("modules"); }
#[test] fn conversions() { run_diff("conversions"); }
#[test] fn unless_basics() { run_diff("unless_basics"); }
#[test] fn regex_minimal() { run_diff("regex_minimal"); }
#[test] fn regex_class_methods() { run_diff("regex_class_methods"); }
#[test] fn splat_block_forwarding() { run_diff("splat_block_forwarding"); }
#[test] fn splat_call_block() { run_diff("splat_call_block"); }
#[cfg(feature = "regex")]
#[test] fn scan_block_last_match() { run_diff("scan_block_last_match"); }
#[cfg(feature = "regex")]
#[test] fn match_data_inspect_named() { run_diff("match_data_inspect_named"); }
#[cfg(feature = "regex")]
#[test] fn match_data_names() { run_diff("match_data_names"); }
#[cfg(feature = "regex")]
#[test] fn string_match_block_pos() { run_diff("string_match_block_pos"); }
#[test] fn bare_universal_primitive_self() { run_diff("bare_universal_primitive_self"); }
#[test] fn string_codepoints() { run_diff("string_codepoints"); }
#[test] fn sprintf_positional() { run_diff("sprintf_positional"); }
#[cfg(feature = "regex")]
#[test] fn string_gsub_enumerator() { run_diff("string_gsub_enumerator"); }
#[cfg(feature = "regex")]
#[test] fn string_gsub_string_pattern() { run_diff("string_gsub_string_pattern"); }
#[test] fn hash_transform_enumerator() { run_diff("hash_transform_enumerator"); }
#[test] fn hash_transform_keys_mapping() { run_diff("hash_transform_keys_mapping"); }
#[test] fn method_bind_call_builtin() { run_diff("method_bind_call_builtin"); }
#[test] fn kernel_hash_conversion() { run_diff("kernel_hash_conversion"); }
#[test] fn integer_sqrt() { run_diff("integer_sqrt"); }
#[test] fn float_to_int() { run_diff("float_to_int"); }
#[test] fn proc_lambda_predicate() { run_diff("proc_lambda_predicate"); }
#[test] fn block_optional_params() { run_diff("block_optional_params"); }
#[test] fn block_arity_optional() { run_diff("block_arity_optional"); }
#[test] fn block_arity_keywords() { run_diff("block_arity_keywords"); }
#[test] fn lambda_local_return() { run_diff("lambda_local_return"); }
#[test] fn lambda_strict_arity() { run_diff("lambda_strict_arity"); }
#[test] fn symbol_proc_multiarg() { run_diff("symbol_proc_multiarg"); }
#[test] fn ensure_on_return() { run_diff("ensure_on_return"); }
#[test] fn raise_class_runs_initialize() { run_diff("raise_class_runs_initialize"); }
#[test] fn time_components() { run_diff("time_components"); }
#[cfg(feature = "stdlib")]
#[test] fn stringio_line_methods() { run_diff("stringio_line_methods"); }
#[test] fn comparable_is_module() { run_diff("comparable_is_module"); }
#[test] fn numeric_remainder_ceildiv() { run_diff("numeric_remainder_ceildiv"); }
#[test] fn lazy_each_with_index() { run_diff("lazy_each_with_index"); }
#[cfg(feature = "stdlib")]
#[test] fn set_subtract_divide() { run_diff("set_subtract_divide"); }
#[test] fn hash_min_by_n() { run_diff("hash_min_by_n"); }
#[test] fn array_values_at_setops() { run_diff("array_values_at_setops"); }
#[test] fn kernel_array_range() { run_diff("kernel_array_range"); }
#[cfg(feature = "regex")]
#[test] fn match_data_offsets() { run_diff("match_data_offsets"); }
#[cfg(feature = "regex")]
#[test] fn regex_named_disables_numbered() { run_diff("regex_named_disables_numbered"); }
#[test] fn array_literal_over_65k() { run_diff("array_literal_over_65k"); }
#[test] fn block_param_trailing_comma() { run_diff("block_param_trailing_comma"); }
#[test] fn require_relative_dotted() { run_diff("require_relative_dotted"); }
#[test] fn class_compact_path_in_scope() { run_diff("class_compact_path_in_scope"); }
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
// String#tr_s (squeeze translated runs) + #sum (byte checksum).
#[test] fn string_tr_s_sum() { run_diff("string_tr_s_sum"); }
// Struct#values_at + #dig.
#[test] fn struct_values_at_dig() { run_diff("struct_values_at_dig"); }
// Set#^ / #disjoint? / #intersect? (stdlib_vendor set.rb).
#[cfg(feature = "stdlib")]
#[test] fn set_xor_disjoint() { run_diff("set_xor_disjoint"); }
// Method#arity = 1 for primitive-backed binary operators.
#[test] fn method_operator_arity() { run_diff("method_operator_arity"); }
// sprintf `*` argument-driven width / precision.
#[test] fn sprintf_star_width() { run_diff("sprintf_star_width"); }
// Enumerator::Lazy#zip.
#[test] fn lazy_zip() { run_diff("lazy_zip"); }
// Inline-constant-cache invalidation (const_set shadow / reopen /
// include / anon naming) — see Vm::const_cache_flat.
#[test] fn const_inline_cache() { run_diff("const_inline_cache"); }
// String#partition/#rpartition (str+regex), #insert, #delete.
#[cfg(feature = "regex")]
#[test] fn string_partition_insert_delete() { run_diff("string_partition_insert_delete"); }
// Struct: keyword_init, block form, to_h/[]/each, inspect.
#[test] fn struct_features() { run_diff("struct_features"); }
#[test] fn struct_subclass_factory() { run_diff("struct_subclass_factory"); }
// Ruby 3.2 Data.define — immutable value objects.
#[test] fn data_define() { run_diff("data_define"); }
// `Set[...]` constructor (stdlib-vendored Set surface).
#[cfg(feature = "stdlib")]
#[test] fn set_bracket_ctor() { run_diff("set_bracket_ctor"); }
// Set#replace (stdlib_vendor set.rb).
#[cfg(feature = "stdlib")]
#[test] fn set_replace() { run_diff("set_replace"); }
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
#[test] fn const_autovivified_module() { run_diff("const_autovivified_module"); }
#[test] fn require_override_autoload() { run_diff("require_override_autoload"); }
#[test] fn require_openssl_zlib_stub() { run_diff("require_openssl_zlib_stub"); }
// Real OpenSSL crypto (HMAC variants, PBKDF2, secure_compare, Digest
// streaming) needs the `_openssl` build; CRuby's core openssl is the
// oracle (loadable under --disable-gems).
#[cfg(feature = "_openssl")]
#[test] fn openssl_crypto() { run_diff("openssl_crypto"); }
// AES-256-GCM authenticated encryption (Cipher) — encrypt/tag,
// verified decrypt, tamper detection.
#[cfg(feature = "_openssl")]
#[test] fn openssl_aes_gcm() { run_diff("openssl_aes_gcm"); }
// AES-256-CBC with PKCS#7 padding — round-trip, padding=0, bad decrypt.
#[cfg(feature = "_openssl")]
#[test] fn openssl_aes_cbc() { run_diff("openssl_aes_cbc"); }
// AES-128 (16-byte key, 10 rounds) across CBC/CTR/GCM.
#[cfg(feature = "_openssl")]
#[test] fn openssl_aes128() { run_diff("openssl_aes128"); }
#[test] fn super_to_primitive() { run_diff("super_to_primitive"); }
#[test] fn fancy_regex_captures() { run_diff("fancy_regex_captures"); }
#[test] fn numeric_comparable() { run_diff("numeric_comparable"); }
#[test] fn call_splat_coerce() { run_diff("call_splat_coerce"); }
#[test] fn call_kwsplat_empty() { run_diff("call_kwsplat_empty"); }
#[test] fn array_reduce_init_sym() { run_diff("array_reduce_init_sym"); }
#[test] fn regexp_options_carrier() { run_diff("regexp_options_carrier"); }
#[cfg(feature = "regex")]
#[test] fn regexp_casefold() { run_diff("regexp_casefold"); }
#[cfg(feature = "regex")]
#[test] fn regexp_match_write() { run_diff("regexp_match_write"); }
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
#[test] fn yield_in_escaped_closure() { run_diff("yield_in_escaped_closure"); }
#[test] fn regex_binary_bytes() { run_diff("regex_binary_bytes"); }
#[test] fn object_singleton_super() { run_diff("object_singleton_super"); }
#[test] fn bare_super_forwarding() { run_diff("bare_super_forwarding"); }
#[test] fn bare_super_implicit_block() { run_diff("bare_super_implicit_block"); }
#[test] fn super_lifecycle_hook_block() { run_diff("super_lifecycle_hook_block"); }
#[test] fn super_to_native_class_method() { run_diff("super_to_native_class_method"); }
#[test] fn class_self_nested_const() { run_diff("class_self_nested_const"); }
#[test] fn module_class_hierarchy() { run_diff("module_class_hierarchy"); }
#[test] fn module_subclass() { run_diff("module_subclass"); }
#[test] fn define_method_optional_params() { run_diff("define_method_optional_params"); }
#[test] fn ruby_version_constants() { run_diff("ruby_version_constants"); }
#[test] fn instance_method_universal() { run_diff("instance_method_universal"); }
#[test] fn class_self_expr_value() { run_diff("class_self_expr_value"); }
#[test] fn const_added_hook() { run_diff("const_added_hook"); }
#[test] fn symbol_with_predicates() { run_diff("symbol_with_predicates"); }
#[test] fn string_affix_variadic() { run_diff("string_affix_variadic"); }
#[test] fn scoped_const_autoload_shadow() { run_diff("scoped_const_autoload_shadow"); }
#[test] fn method_getter_string_arg() { run_diff("method_getter_string_arg"); }
#[test] fn bare_identity_methods() { run_diff("bare_identity_methods"); }
#[test] fn hash_merge_variadic() { run_diff("hash_merge_variadic"); }
#[test] fn name_error_two_arg() { run_diff("name_error_two_arg"); }
#[test] fn loaded_features_completion_order() { run_diff("loaded_features_completion_order"); }
#[test] fn proc_parameters() { run_diff("proc_parameters"); }
#[test] fn define_method_from_builtin() { run_diff("define_method_from_builtin"); }
#[test] fn io_class_read() { run_diff("io_class_read"); }
#[test] fn fused_local_recv_call() { run_diff("fused_local_recv_call"); }
#[test] fn proc_call_fastpath() { run_diff("proc_call_fastpath"); }
#[test] fn method_lookup_lazy_set() { run_diff("method_lookup_lazy_set"); }
#[test] fn ivar_table() { run_diff("ivar_table"); }
#[test] fn hash_literal_dedup() { run_diff("hash_literal_dedup"); }
#[test] fn throw_no_backtrace() { run_diff("throw_no_backtrace"); }
#[test] fn dir_pwd_cache() { run_diff("dir_pwd_cache"); }
#[test] fn norecv_self_dispatch() { run_diff("norecv_self_dispatch"); }
#[test] fn attr_reader_getter_fastpath() { run_diff("attr_reader_getter_fastpath"); }
#[test] fn array_new_block_gc_root() { run_diff("array_new_block_gc_root"); }
#[test] fn hash_shift_gc_root() { run_diff("hash_shift_gc_root"); }
#[test] fn gc_stat() { run_diff("gc_stat"); }
#[cfg(feature = "stdlib")]
#[test] fn kernel_printf() { run_diff("kernel_printf"); }
#[test] fn kwsplat_empty_forwarding() { run_diff("kwsplat_empty_forwarding"); }
#[test] fn block_brace_hash_and_kwsplat() { run_diff("block_brace_hash_and_kwsplat"); }
#[cfg(feature = "regex")]
#[test] fn regexp_symbol_arg() { run_diff("regexp_symbol_arg"); }
#[test] fn module_class_variable_reflection() { run_diff("module_class_variable_reflection"); }
#[test] fn super_is_a() { run_diff("super_is_a"); }
#[test] fn regexp_names() { run_diff("regexp_names"); }
#[cfg(feature = "regex")]
#[test] fn regexp_named_captures() { run_diff("regexp_named_captures"); }
#[test] fn objectspace_finalizer() { run_diff("objectspace_finalizer"); }
#[test] fn hash_subclass_default() { run_diff("hash_subclass_default"); }
#[test] fn hash_subclass_transform_override() { run_diff("hash_subclass_transform_override"); }
#[test] fn clone_preserves_singleton() { run_diff("clone_preserves_singleton"); }
#[test] fn dig_typeerror_nondiggable() { run_diff("dig_typeerror_nondiggable"); }
#[test] fn hash_native_instance_methods() { run_diff("hash_native_instance_methods"); }
#[test] fn case_subject_eval_once() { run_diff("case_subject_eval_once"); }
#[test] fn string_slice_utf8_invalid() { run_diff("string_slice_utf8_invalid"); }
#[test] fn file_path_coercion() { run_diff("file_path_coercion"); }
#[test] fn regexp_match_pos() { run_diff("regexp_match_pos"); }
#[test] fn regexp_match_nil() { run_diff("regexp_match_nil"); }
#[test] fn regexp_binary_capture() { run_diff("regexp_binary_capture"); }
#[cfg(feature = "stdlib")]
#[test] fn strscan_binary_capture() { run_diff("strscan_binary_capture"); }
#[test] fn string_match_binary() { run_diff("string_match_binary"); }
#[test] fn regexp_union_flags() { run_diff("regexp_union_flags"); }
#[test] fn regexp_dup_named_capture() { run_diff("regexp_dup_named_capture"); }
#[test] fn exception_inherits_object() { run_diff("exception_inherits_object"); }
#[test] fn pipe_write_closed_read() { run_diff("pipe_write_closed_read"); }
#[test] fn string_ascii_index() { run_diff("string_ascii_index"); }
#[cfg(feature = "stdlib")]
#[test] fn strscan_scan_until() { run_diff("strscan_scan_until"); }
#[cfg(feature = "stdlib")]
#[test] fn strscan_linear_scaling() { run_diff("strscan_linear_scaling"); }
#[cfg(feature = "stdlib")]
#[test] fn strscan_anchored_match() { run_diff("strscan_anchored_match"); }
#[test] fn string_ascii_only() { run_diff("string_ascii_only"); }
#[cfg(feature = "stdlib")]
#[test] fn query_parse_linear_scaling() { run_diff("query_parse_linear_scaling"); }
#[test] fn multi_assign_constants() { run_diff("multi_assign_constants"); }
#[test] fn rbconfig_interpreter() { run_diff("rbconfig_interpreter"); }
#[cfg(feature = "stdlib")]
#[test] fn singleton_mixin() { run_diff("singleton_mixin"); }
#[cfg(feature = "stdlib")]
#[test] fn fileutils_reflection() { run_diff("fileutils_reflection"); }
#[test] fn class_try_convert() { run_diff("class_try_convert"); }
#[test] fn toplevel_self_main() { run_diff("toplevel_self_main"); }
#[test] fn visibility_method_explicit_recv() { run_diff("visibility_method_explicit_recv"); }
#[test] fn exception_cause() { run_diff("exception_cause"); }
#[test] fn bare_warn_singleton_override() { run_diff("bare_warn_singleton_override"); }
#[test] fn dup_clone_initialize_copy() { run_diff("dup_clone_initialize_copy"); }
#[test] fn binary_string_length_replace() { run_diff("binary_string_length_replace"); }
#[test] fn sub_gsub_byte_faithful() { run_diff("sub_gsub_byte_faithful"); }
#[cfg(feature = "stdlib")]
#[test] fn stringio_binary_read() { run_diff("stringio_binary_read"); }
#[cfg(feature = "stdlib")]
#[test] fn tempfile_binmode() { run_diff("tempfile_binmode"); }
#[cfg(feature = "stdlib")]
#[test] fn tempfile_to_path() { run_diff("tempfile_to_path"); }
#[test] fn undef_inherited_singleton() { run_diff("undef_inherited_singleton"); }
#[test] fn frame_locals_pool() { run_diff("frame_locals_pool"); }
#[test] fn locals_stack_arena() { run_diff("locals_stack_arena"); }
#[test] fn class_singleton_fast_path() { run_diff("class_singleton_fast_path"); }
#[test] fn block_locals_share() { run_diff("block_locals_share"); }
#[test] fn str_hash_cache() { run_diff("str_hash_cache"); }
#[test] fn fxhash_internal_maps() { run_diff("fxhash_internal_maps"); }
#[test] fn fxhash_vm_maps() { run_diff("fxhash_vm_maps"); }
#[test] fn inline_cache_dispatch() { run_diff("inline_cache_dispatch"); }
#[test] fn p0_each_yield_composability() { run_diff("p0_each_yield_composability"); }
#[test] fn string_dump() { run_diff("string_dump"); }
#[test] fn string_count() { run_diff("string_count"); }
#[test] fn string_ord() { run_diff("string_ord"); }
#[test] fn string_byteslice() { run_diff("string_byteslice"); }
#[test] fn integer_parse_bases() { run_diff("integer_parse_bases"); }
#[test] fn warn_uplevel() { run_diff("warn_uplevel"); }
#[cfg(feature = "stdlib")]
#[test] fn zlib_streaming() { run_diff("zlib_streaming"); }
#[cfg(feature = "stdlib")]
#[test] fn uri_decode_www_form() { run_diff("uri_decode_www_form"); }
#[cfg(feature = "stdlib")]
#[test] fn uri_parse_invalid_authority() { run_diff("uri_parse_invalid_authority"); }
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
#[test] fn rational_methods() { run_diff("rational_methods"); }
#[test] fn complex_numbers() { run_diff("complex_numbers"); }
#[test] fn numeric_complex_protocol() { run_diff("numeric_complex_protocol"); }
#[test] fn complex_protocol_extras() { run_diff("complex_protocol_extras"); }
#[test] fn integer_bit_range() { run_diff("integer_bit_range"); }
#[test] fn float_adjacent() { run_diff("float_adjacent"); }
#[test] fn numeric_coerce_protocol() { run_diff("numeric_coerce_protocol"); }
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
// Time.parse accepts RFC 2822 / RFC 7231 httpdate (month-name) shapes,
// not just ISO — rack Response cache helpers re-parse their own
// httpdate output. All cases carry an explicit zone (TZ-independent).
#[test] fn time_parse_rfc() { run_diff("time_parse_rfc"); }
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
#[test] fn unpack_base64_strict() { run_diff("unpack_base64_strict"); }
// Ruby-level Fiber class API (Fiber.new/#resume/Fiber.yield/#alive?/
// FiberError) — needs the `_fiber` build; CRuby has Fiber natively.
#[cfg(feature = "_fiber")]
#[test] fn fiber_api() { run_diff("fiber_api"); }
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
#[test] fn massign_expr_value() { run_diff("massign_expr_value"); }
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
#[cfg(unix)]
#[test] fn fileutils_ln_s() { run_diff("fileutils_ln_s"); }
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
// BigDecimal ROUND_* constants + mode-aware #round, finite-state
// predicates / #sign, and bigdecimal/util #to_d. Surfaced by money.
#[cfg(feature = "stdlib")]
#[test] fn bigdecimal_modes_util() { run_diff_gem("bigdecimal_modes_util", "bigdecimal"); }
// BigDecimal < Numeric ancestry + the inherited real-number complex
// protocol and Rational-derived numerator/denominator/fdiv.
#[cfg(feature = "stdlib")]
#[test] fn bigdecimal_numeric_protocol() { run_diff_gem("bigdecimal_numeric_protocol", "bigdecimal"); }
// Date method surface: commercial date, step/upto/downto, ld/mjd,
// inspect tuple format, strptime. Oracle = CRuby's core `date`.
#[cfg(feature = "stdlib")]
#[test] fn date_methods() { run_diff_gem("date_methods", "date"); }
// DateTime: time-preserving +/-, offset/zone/sec_fraction/new_offset,
// iso8601/strptime parsers, UTC-instant inspect tuple.
#[cfg(feature = "stdlib")]
#[test] fn datetime_methods() { run_diff_gem("datetime_methods", "date"); }
// IPAddr: succ / private? / loopback? + to_range endpoints.
#[cfg(feature = "stdlib")]
#[test] fn ipaddr_methods() { run_diff_gem("ipaddr_methods", "ipaddr"); }
// `Monitor` autoloaded (available without explicit require, as in a
// full Ruby env). Oracle is gem-enabled CRuby (`--disable=gems` lacks
// the ambient Monitor). Surfaced by dotenv's bare `Monitor.new`.
#[cfg(feature = "stdlib")]
#[test] fn monitor_autoload() { run_diff_gem("monitor_autoload", "monitor"); }
#[test] fn gsub_hash() { run_diff("gsub_hash"); }
#[test] fn hash_delete_block() { run_diff("hash_delete_block"); }
// Hash#flatten(level) / #fetch_values (KeyError on miss) /
// #compare_by_identity? — rack Headers supers into these.
#[test] fn hash_flatten_fetch_values() { run_diff("hash_flatten_fetch_values"); }
// Hash#freeze enforcement (twin of array_freeze): every mutator
// (incl. []=, block forms) raises FrozenError; clone preserves frozen,
// dup resets.
#[test] fn hash_freeze() { run_diff("hash_freeze"); }
#[test] fn string_casecmp() { run_diff("string_casecmp"); }
// Case methods (upcase/downcase/capitalize/swapcase + `!`) raise
// ArgumentError "input string invalid" on encoding-invalid receivers
// (CRuby); valid strings convert. rack MethodOverride upcase-rescue.
#[test] fn case_invalid_encoding() { run_diff("case_invalid_encoding"); }
// A frozen String's mutation FrozenError renders the receiver's
// inspect (`"y"`), not raw bytes — shared with String#inspect.
#[test] fn string_frozen_message() { run_diff("string_frozen_message"); }
// String#split (regex + literal sep) preserves bytes + encoding for
// BINARY / invalid-UTF-8 receivers (no U+FFFD mangle). rack
// QueryParser `_method=\xBF` → MethodOverride upcase-raise.
#[test] fn string_split_binary() { run_diff("string_split_binary"); }
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
// Array#rindex value + block forms (minitest backtrace filter).
#[test] fn array_rindex() { run_diff("array_rindex"); }
// rescue-splat filters (`rescue *CONST` / `rescue *local`) — the
// minitest PASSTHROUGH_EXCEPTIONS / assert_raises shapes. Before
// these forms existed the splat was dropped and the clause matched
// every StandardError.
#[test] fn rescue_splat_filters() { run_diff("rescue_splat_filters"); }
// sprintf `%s` dispatches user to_s overrides (minitest failure reports).
#[test] fn sprintf_user_to_s() { run_diff("sprintf_user_to_s"); }
// any?/all?/none?/one? with a pattern argument (`pat === element`).
#[cfg(feature = "regex")]
#[test] fn enumerable_pattern_predicates() { run_diff("enumerable_pattern_predicates"); }
// Kernel#puts/print/p/warn route through a reassigned $stdout/$stderr
// (minitest capture_io). Needs the vendored StringIO → stdlib-gated.
#[cfg(feature = "stdlib")]
#[test] fn stdio_redirect_capture() { run_diff("stdio_redirect_capture"); }
// throw flies past intervening StandardError rescues (minitest
// assert_throws); wrong-tag throw raises UncaughtThrowError at site.
#[test] fn throw_passthrough_rescue() { run_diff("throw_passthrough_rescue"); }
#[test] fn throw_past_rescue_exception() { run_diff("throw_past_rescue_exception"); }
// raise-SomeClass message default, SyntaxError class, block-frame names.
#[cfg(feature = "regex")]
#[test] fn exception_surface_extras() { run_diff("exception_surface_extras"); }
// Array#join recurses into nested arrays; cycle-safe.
#[test] fn array_join_recursive() { run_diff("array_join_recursive"); }
// Time.utc/gm/local/mktime civil constructors (Tier-1 UTC-only).
#[test] fn time_civil_constructors() { run_diff("time_civil_constructors"); }
// respond_to? surface: String#=~, Kernel-private include_all bits.
#[test] fn respond_to_surface() { run_diff("respond_to_surface"); }
// Object.const_set installs at toplevel under the bare name —
// minitest's RuntimeError remove/restore round-trip.
#[test] fn const_set_toplevel_roundtrip() { run_diff("const_set_toplevel_roundtrip"); }
// chunk_while / slice_when (materialized-Array Tier-1 shape).
#[test] fn enumerable_chunk_slice_when() { run_diff("enumerable_chunk_slice_when"); }
// Array#* (repetition + join alias), Object#singleton_class on
// instances (class<<self;self;end in methods), Proc#call(&blk).
#[test] fn array_mul_and_eigen() { run_diff("array_mul_and_eigen"); }
// Anonymous-class closure captures / class-ivar GC roots (minitest
// Spec registry shape). Meaningful under the STRESS_GC=1 job.
#[test] fn anon_class_closure_gc() { run_diff("anon_class_closure_gc"); }
// public/private/protected_method_defined? (minitest Spec nested-it).
#[test] fn method_defined_visibility() { run_diff("method_defined_visibility"); }
// Array#delete_at value/negative/out-of-range forms.
#[test] fn array_delete_at() { run_diff("array_delete_at"); }
// Vendored OptionParser (minitest process_args surface) — stdlib-gated.
#[cfg(feature = "stdlib")]
#[test] fn optparse_minitest_surface() { run_diff("optparse_minitest_surface"); }
// Marshal same-process round-trip contract + dumpability TypeErrors
// + Exception ivar reflection (message/backtrace hidden).
#[test] fn marshal_roundtrip_contract() { run_diff("marshal_roundtrip_contract"); }
// Real binary Marshal.dump (common-tag subset): deep copy, CRuby-4.8
// byte compatibility, shared-object links, cycles, encoding round-trip.
#[test] fn marshal_binary_dump() { run_diff("marshal_binary_dump"); }
// Struct (`S`-tag) marshalling: byte-compatible dump, deep copy,
// nested/shared structs, anonymous-struct token fallback.
#[test] fn marshal_struct() { run_diff("marshal_struct"); }
// Generic object (`o`-tag) + exception (`:mesg`/`:bt`) marshalling:
// byte-compatible dump, deep copy, exception state + subclass round-trip.
#[test] fn marshal_object() { run_diff("marshal_object"); }
// `}` (Hash-with-default) + `C` (Array/Hash subclass wrapper) tags.
#[test] fn marshal_hash_default_subclass() { run_diff("marshal_hash_default_subclass"); }
// `u` (_dump/_load) + `U` (marshal_dump/marshal_load) user hooks —
// reentrant Ruby calls from the serializer; exceptions propagate.
#[test] fn marshal_user_hooks() { run_diff("marshal_user_hooks"); }
// Numeric to_s/inspect output is US-ASCII (CRuby), not UTF-8.
#[test] fn numeric_to_s_encoding() { run_diff("numeric_to_s_encoding"); }
// nil/true/false to_s/inspect + Symbol to_s/name/inspect encoding.
#[test] fn nil_bool_symbol_encoding() { run_diff("nil_bool_symbol_encoding"); }
// Array/Hash/Range to_s/inspect encoding: seed from first element,
// promote to UTF-8 on non-ASCII (CRuby's quirky rule).
#[test] fn collection_inspect_encoding() { run_diff("collection_inspect_encoding"); }
// `const_missing` hook fires on a missing constant before NameError.
#[test] fn const_missing_hook() { run_diff("const_missing_hook"); }
// Comparable failure message: "comparison of <class> with <other> failed".
#[test] fn comparison_failed_message() { run_diff("comparison_failed_message"); }
// Object#clone(freeze: true|false|nil) override (dup rejects it).
#[test] fn clone_freeze_kwarg() { run_diff("clone_freeze_kwarg"); }
// %a / %A C99 hexadecimal float sprintf.
#[test] fn sprintf_hex_float() { run_diff("sprintf_hex_float"); }
// UAX#29 grapheme clusters + UCD normalization (non-ASCII needs the
// unicode-* crates behind _encoding_full).
#[cfg(feature = "_encoding_full")]
#[test] fn unicode_grapheme_normalize() { run_diff("unicode_grapheme_normalize"); }
// Subclassing String (class_tag): content + methods + ivars + override.
#[test] fn string_subclass() { run_diff("string_subclass"); }
// Range#map over String endpoints (str_succ materialize).
#[test] fn range_string_map() { run_diff("range_string_map"); }
// undef_method kills same-class methods (tombstone + table removal).
#[test] fn undef_own_class_method() { run_diff("undef_own_class_method"); }
// super from overrides into Object#send/=== — UNGATED so the
// default-features Coverage job exercises the ApplySuperBlock
// fallback lines in step.rs (stdlib-gating it dropped step.rs
// below the per-file ratchet).
#[test] fn super_into_dispatch_builtins() { run_diff("super_into_dispatch_builtins"); }
// Array#join user-to_s dispatch; Proc inspect file:line form.
#[cfg(feature = "regex")]
#[test] fn join_proc_inspect_dispatch() { run_diff("join_proc_inspect_dispatch"); }
// Invalid-UTF-8 inspect: per-byte \xNN for bad runs (mu_pp headers).
#[test] fn string_invalid_utf8_inspect() { run_diff("string_invalid_utf8_inspect"); }
// class << self inside a method body (runtime-self eigenclass).
#[test] fn class_lt_lt_self_in_method() { run_diff("class_lt_lt_self_in_method"); }
// alias/restore of VM-side lifecycle hook defaults (Class#inherited).
#[test] fn lifecycle_hook_alias() { run_diff("lifecycle_hook_alias"); }
// Anonymous class display serial (#<Class:0xN> shape).
#[cfg(feature = "regex")]
#[test] fn anon_class_display_serial() { run_diff("anon_class_display_serial"); }
// Kernel#binding + raise-time set_backtrace dispatch + hook super chain.
#[test] fn binding_set_backtrace_raise() { run_diff("binding_set_backtrace_raise"); }
// Math module over the __rubyrs_math primitive (+ aliasability).
#[test] fn math_module_surface() { run_diff("math_module_surface"); }
// raise as a method: send-form, eigenclass stub of bare raise, Symbol#=~.
#[cfg(feature = "regex")]
#[test] fn raise_as_method() { run_diff("raise_as_method"); }
// String class_eval runs in receiver class context; Regexp ==;
// anon-instance inspect nesting.
#[cfg(feature = "regex")]
#[test] fn class_eval_string_context() { run_diff("class_eval_string_context"); }
// defined?(name) in class bodies via the class-object chain; Proc eq.
#[cfg(feature = "regex")]
#[test] fn defined_method_in_class_body() { run_diff("defined_method_in_class_body"); }
// Hash per-instance eigenclass (def h.method_missing, overrides).
#[test] fn hash_singleton_methods() { run_diff("hash_singleton_methods"); }
// system/backtick capability + Tempfile diff pipeline — stdlib-gated
// (vendored Tempfile) and regex-gated (sub! patterns).
#[cfg(all(feature = "stdlib", feature = "regex"))]
#[test] fn process_spawn_diff_pipeline() { run_diff("process_spawn_diff_pipeline"); }
// $stdout.reopen(tempfile) delegation (capture_subprocess_io).
#[cfg(feature = "stdlib")]
#[test] fn io_reopen_capture() { run_diff("io_reopen_capture"); }
// Block keyword params: |k1:, k2:| required + |k: default| optional,
// CRuby error wording/ordering, kwargs-vs-positional-Hash recovery.
#[test] fn block_kw_params() { run_diff("block_kw_params"); }
// NoMethodError receiver shapes (nil/true/instance-of/class/module)
// + undef_method/alias_method NameError naming on eigenclass shells.
#[test] fn nomethod_receiver_shapes() { run_diff("nomethod_receiver_shapes"); }
// Kernel#sleep user-override gate (minitest stubs sleep on tests).
#[test] fn kernel_sleep_override() { run_diff("kernel_sleep_override"); }
// Blank-slate dispatch family: alias-of-builtin snapshots,
// instance_methods universals, undef->method_missing,
// redefine-after-undef, public_send override.
#[test] fn blank_slate_dispatch() { run_diff("blank_slate_dispatch"); }
// proc.call(args, &blk) binds the callee's |.., &b| slot.
#[test] fn proc_call_block_arg() { run_diff("proc_call_block_arg"); }
// super->method_missing fallback, caller(Range), bare reflection
// universals (minitest Object#stub family).
#[test] fn super_mm_caller_range() { run_diff("super_mm_caller_range"); }
// String per-instance eigenclass (def s.foo / stub save-restore).
#[test] fn string_singleton_methods() { run_diff("string_singleton_methods"); }
// Default-inspect ivar tail, %p dispatch (incl. container elements),
// str-singleton operator gate, =~ to_str, pattern length-mismatch.
#[test] fn inspect_ivars_pattern_msg() { run_diff("inspect_ivars_pattern_msg"); }
// Class kind_of? extend-chain + Thread.new empty fiber-locals.
#[test] fn class_kind_of_extend_thread_locals() { run_diff("class_kind_of_extend_thread_locals"); }
// system under $stdout/$stderr reopen-delegation (subprocess capture).
#[cfg(feature = "stdlib")]
#[test] fn system_capture_redirect() { run_diff("system_capture_redirect"); }
// Real fork(2) + waitpid + $? (block form; unix; spawn-gated).
#[cfg(unix)]
#[test] fn fork_waitpid_status() { run_diff("fork_waitpid_status"); }
// Marshal.load over real CRuby 4.8 bytes (common-tag subset) +
// binary-mode whole-buffer handle read transparency.
#[test] fn marshal_load_binary() { run_diff("marshal_load_binary"); }
// rack self-suite batch 1: defined?(recv.m) guards, public-flip of
// inherited module_function methods, def-self.x stays public,
// remove_method eigenclass bridge, bare warn/fail user override,
// Hash-subclass super into initialize, respond_to? super, Regexp
// encoding-flag constants. (zero require — runs in every build.)
#[test] fn rack_spec_vm_fixes() { run_diff("rack_spec_vm_fixes"); }
// rack self-suite batch 3 core (zero require): Hash assoc/rassoc/
// shift/value?/select!/keep_if/reject!/delete_if, String slice!/
// index(regexp)/rindex(off)/scrub!, dual-engine captures, undef in
// instance_eval, Struct-subclass member resolution (+ the GC
// superclass-chain root-hole reproducer).
#[test] fn string_hash_core_ops() { run_diff("string_hash_core_ops"); }
// rack self-suite batch 2 library surface (requires stringio /
// tempfile / yaml / cgi/cookie / time): IO#read(len, outbuf),
// Tempfile byte reads, IO.pipe, YAML round-trip, CGI::Cookie,
// Time.httpdate parse + utc flavour.
#[cfg(feature = "stdlib")]
#[test] fn rack_spec_lib_fixes() { run_diff("rack_spec_lib_fixes"); }
// File class predicates (zero require, covers vm/fileops.rs):
// File.readable?/writable?/executable?/size?/mtime over a real /tmp
// file. Rack::Files gates serving on file?/readable? and emits
// Last-Modified from mtime; clears spec_files/spec_static/spec_cascade.
#[test] fn file_predicate_methods() { run_diff("file_predicate_methods"); }
// BINARY String#[] byte-slicing (zero require, covers vm/string.rs):
// ASCII-8BIT receivers index by bytes + keep the tag, instead of the
// UTF-8-lossy char view that mangled StringIO/Zlib over binary data.
#[test] fn string_binary_slice() { run_diff("string_binary_slice"); }
// eval(src, binding) self-dispatch: Kernel#binding captures self,
// eval runs with it (method + ivar dispatch). rack Builder.new_from_string.
#[test] fn eval_binding_self() { run_diff("eval_binding_self"); }
// eval(src, binding) local-variable capture: Kernel#binding snapshots
// the caller's named locals; eval re-seeds them (lambda-wrap parse).
// rack ShowExceptions/ShowStatus ERB `template.result(binding)`.
#[test] fn eval_binding_lvar() { run_diff("eval_binding_lvar"); }
// eval(src, binding) preserves the source's line numbers (the
// locals-capturing lambda wrap goes on the source's first line, not a
// new one) + strips a leading BOM. rack Builder.parse_file __LINE__.
#[test] fn eval_binding_line() { run_diff("eval_binding_line"); }
// Per-instance singleton methods on Array / Proc (heap_singletons
// side-table): define_singleton_method + `def obj.x`, per-instance,
// native dispatch intact. rack Deflater/Lock/ContentLength define
// :close/:each on Array/Proc bodies.
#[test] fn singleton_method_builtins() { run_diff("singleton_method_builtins"); }
// Proc#singleton_class / Array#singleton_class → per-instance eigenclass
// (heap_singletons); class_eval installs methods + aliases native #call.
// rack spec_response `body.singleton_class.class_eval{alias << call}`.
#[test] fn heap_singleton_class() { run_diff("heap_singleton_class"); }
// Zlib veneer over flate2 (stdlib): gzip/deflate round-trips,
// GzipWriter/Reader, auto-inflate. rack Deflater + Static.
#[cfg(feature = "stdlib")]
#[test] fn zlib_roundtrip() { run_diff("zlib_roundtrip"); }
// ERB (vendored erb.rb + verbatim erb/compiler.rb): ERB.new(str)
// .result(binding) — template reads the handler's locals via the
// captured binding + calls a method on the captured self. rack
// ShowExceptions / ShowStatus render their HTML this way.
#[cfg(feature = "stdlib")]
#[test] fn erb_render() { run_diff("erb_render"); }

// Ruby 3.1 hash/keyword value-omission shorthand `{x:}` / `foo(x:)`.
// Surfaced by bridgetown-core, which uses the shorthand heavily.
#[test] fn hash_value_shorthand() { run_diff("hash_value_shorthand"); }

// `methods(false)` / `singleton_methods(false)` — optional regular/all
// boolean restricts to the receiver's own methods. Surfaced by stdlib
// fileutils.rb's `private_instance_methods & methods(false)` table build.
#[test] fn methods_regular_arg() { run_diff("methods_regular_arg"); }

// `# shareable_constant_value` magic comment (ShareableConstantNode):
// no Ractor model, so the wrapper unwraps to the inner constant write.
// Surfaced by stdlib time.rb.
#[test] fn shareable_constant_value() { run_diff("shareable_constant_value"); }

// `Module.instance_method(:name).bind_call(mod)` — native Module#name is
// exposed through reflection so zeitwerk's RealModName can capture it.
#[test] fn module_name_reflection() { run_diff("module_name_reflection"); }

// `Dir.each_child(path)` — block + Enumerator forms, excluding "."/"..".
// zeitwerk's Loader::Helpers walks autoload directories this way.
#[test] fn dir_each_child() { run_diff("dir_each_child"); }

// A required file's body runs at top-level lexical nesting: its top-level
// `def`s become global functions even when the `require` sits inside a
// class body. Surfaced by mustermann's `require 'delegate'` inside
// Hanami::Router (DelegateClass must be global, not a Router method).
#[test] fn require_inside_class_body() { run_diff("require_inside_class_body"); }

// `const_get(name, false)` fires a registered autoload through a user
// `Kernel#require` override — zeitwerk's eager_load descends implicit-
// namespace directories this way (Bridgetown / Hanami boot path).
#[test] fn const_get_autoload_override() { run_diff("const_get_autoload_override"); }

// `Kernel.method_defined?` answers honestly (not blanket-true for the
// Kernel sentinel) so the `alias_method … unless method_defined?` guard
// idiom works — zeitwerk's require wrapper and require-intercepting shims.
#[test] fn kernel_method_defined() { run_diff("kernel_method_defined"); }

// `class << self` whose defs are wrapped in an if/ELSIF/else or case/when
// chain — routed to the real eigenclass-body op (the desugar bails on
// elsif). Surfaced by listen's MonotonicTime on the Bridgetown boot path.
#[test] fn singleton_class_elsif_case_def() { run_diff("singleton_class_elsif_case_def"); }

// `super(key, ...)` — Ruby 3.0 argument forwarding in an explicit-args
// super call. Surfaced by faraday's Utils::Headers#fetch.
#[test] fn super_forwarding_args() { run_diff("super_forwarding_args"); }

// String-form `class_eval` / `module_eval` captures the caller's local
// binding (bare identifiers resolve to enclosing-method locals) while
// `def` still installs onto the receiver class. Surfaced by faraday's
// Options.memoized.
#[test] fn class_eval_string_locals() { run_diff("class_eval_string_locals"); }

// `ruby2_keywords(:m)` is a no-op (rubyrs already collects trailing
// kwargs into the rest param); returns nil. Surfaced by faraday's
// RackBuilder::Handler.
#[test] fn ruby2_keywords_noop() { run_diff("ruby2_keywords_noop"); }

// Reopening a `Struct.new`-created class assigned to a constant inside a
// module reopens the SAME class (scoped name/key), preserving members,
// extend'd singleton methods, and instance methods. Surfaced by faraday's
// `Request = Struct.new(…){ extend MiddlewareRegistry }` + reopen.
#[test] fn reopen_struct_in_module() { run_diff("reopen_struct_in_module"); }

// `super(*a, &b)` from a `new` defined in an EXTENDED module reaches the
// builtin Class#new (allocate + initialize, block forwarded). Surfaced by
// concurrent-ruby's SafeInitialization on Concurrent::Delay.
#[test] fn super_class_new_extended_module() { run_diff("super_class_new_extended_module"); }

// A missing multi-segment require (`require "foo/bar"`) raises LoadError
// instead of being lenient-satisfied by a same-named top-level module.
// concurrent-ruby's native loader relies on the LoadError to pick its
// pure-Ruby fallback.
#[test] fn require_missing_subpath_loaderror() { run_diff("require_missing_subpath_loaderror"); }

// `String#chop` — \r\n pair / last UTF-8 char / empty-safe; non-mutating.
// Surfaced by net/protocol's readline (ADR 0028 Phase 1 prerequisite).
#[test] fn string_chop() { run_diff("string_chop"); }

// `String#clear` — in-place empty, returns self, keeps encoding,
// FrozenError-aware. Surfaced by net/protocol's rbuf_flush.
#[test] fn string_clear() { run_diff("string_clear"); }

// `Errno::EALREADY` / `ECONNABORTED` — the two socket Errno classes
// rubyrs was missing from faraday-net_http's exception list.
#[test] fn errno_ealready_econnaborted() { run_diff("errno_ealready_econnaborted"); }

// `alias` inside `class << <Const>` / `class << <obj>` (non-self
// singleton receiver) routes to the real eigenclass body. Surfaced by
// stdlib net/http.rb (`class << HTTP; alias …`).
#[test] fn class_lt_lt_const_alias() { run_diff("class_lt_lt_const_alias"); }

// `Module#const_defined?(name, false)` — own-only (no ancestor walk).
// Surfaced by stdlib uri/common.rb's `remove_const … if const_defined?(…, false)`.
#[test] fn const_defined_inherit_false() { run_diff("const_defined_inherit_false"); }

// `String#delete!` — destructive delete (in place, self|nil, FrozenError);
// `delete`/`delete!` whitelisted. Surfaced by stdlib uri/generic.rb.
#[test] fn string_delete_bang() { run_diff("string_delete_bang"); }

// `Module#===` honours included modules (≡ is_a?), not just the
// superclass chain. Surfaced by net/http's `if URI === uri` (URI::Generic
// includes URI).
#[test] fn module_case_equality_include() { run_diff("module_case_equality_include"); }

// `require "io/console"` — lenient load-time stdlib stub (returns true
// then false, `IO` stays defined). Surfaced by the `console` gem
// (samovar → bridgetown CLI).
#[test] fn require_io_console() { run_diff("require_io_console"); }

// `singleton_class.send :alias_method, :[], :new` — aliasing the
// class-level builtin `new`/`allocate` into a singleton method.
// Surfaced by concurrent-ruby's LockFreeStack::Node (`Node[nil, nil]`).
#[test] fn singleton_alias_class_new() { run_diff("singleton_alias_class_new"); }

// `super` from an overridden `include`/`extend` reaching the builtin
// `Module#include`. Surfaced by concurrent-ruby's `Concurrent::ReInclude`
// (Bridgetown boot path).
#[test] fn super_module_include() { run_diff("super_module_include"); }

// Ruby 3.x anonymous splat forwarding (`def m(*); yield(*); end` /
// `other(*)`). Surfaced by bridgetown-core's erb_templates.rb.
#[test] fn anon_splat_forward() { run_diff("anon_splat_forward"); }

// `using M` activates refinements inherited via `include`, not just
// those defined directly in M. Surfaced by bridgetown-foundation's
// `Bridgetown::Refinements` (includes the refine-holding modules).
#[test] fn using_refinement_via_include() { run_diff("using_refinement_via_include"); }

// Reopening `module X`/`class X` with a pending autoload fires the
// autoload first (CRuby semantics). Surfaced by bridgetown-foundation's
// zeitwerk-autoloaded `RefineExt` namespace.
#[test] fn reopen_fires_autoload() { run_diff("reopen_fires_autoload"); }

// `Mod.const_get(:Hash, false)` fires Mod's own pending autoload rather
// than returning the toplevel `::Hash`. Surfaced by zeitwerk eager_load
// over namespaces whose files shadow core class names.
#[test] fn const_get_prefers_local_autoload() { run_diff("const_get_prefers_local_autoload"); }

// `const_get(:Absent)` invokes the receiver's `const_missing(sym)` hook
// before raising NameError. Surfaced by regexp_parser's version_lookup
// (`const_get("V3_4_0")` falling back to the nearest defined version).
#[test] fn const_get_const_missing() { run_diff("const_get_const_missing"); }

// `Module#module_exec` / `#class_exec` — block-with-args twin of
// class_eval's block form (runs in the class body context, block gets
// the explicit args). Also exercises `|*a|` splat capture and the
// block form returning the block's value. Surfaced by rspec building
// example groups via `klass.module_exec(*args, &block)`.
#[test] fn module_exec_class_exec() { run_diff("module_exec_class_exec"); }

// Bare `super` from a method with keyword params forwards them AS
// keywords (reconstructed trailing kwargs Hash), not as positional
// args. Surfaced by public_suffix's `Wildcard#initialize(value:,
// length:, private:); super; end`.
#[test] fn super_forward_kwargs() { run_diff("super_forward_kwargs"); }

// `IO::SEEK_SET` / `SEEK_CUR` / `SEEK_END` whence constants, and
// File#seek honoring them. Surfaced by mini_mime's PReadFile#pread.
#[test] fn io_seek_constants() { run_diff("io_seek_constants"); }

// Fiber storage API (Ruby 3.2+): `Fiber[]` / `Fiber[]=`, backed by one
// process-global store in the single-fiber model. Surfaced by
// multi_json caching its adapter override in `Fiber[:multi_json_adapter]`.
#[test] fn fiber_storage() { run_diff("fiber_storage"); }

// ObjectSpace::WeakMap map API (identity keys; strong-ref Tier-1
// divergence on the weak collection). Surfaced by connection_pool's
// `INSTANCES = ObjectSpace::WeakMap.new`.
#[test] fn objectspace_weakmap() { run_diff("objectspace_weakmap"); }
// GC no-op module: start/enable/disable/count return CRuby's values
// (rubyrs has no user-triggerable collector). Lets `GC.start` /
// `GC.disable` in gem benchmarks/teardown run instead of NameError.
#[test] fn gc_noop_module() { run_diff("gc_noop_module"); }

// Thread::Mutex / Thread::ConditionVariable nested-constant aliases and
// Thread.handle_interrupt (no-op block runner in the single-thread
// model). Surfaced by connection_pool's TimedStack + #with.
#[test] fn thread_nested_consts_handle_interrupt() { run_diff("thread_nested_consts_handle_interrupt"); }

// Keyword args in eval'd code: the eval body runs synchronously inside
// the native `eval` dispatch, which left the trailing-hash-positional
// flag stale-TRUE, so kwarg calls bound positionally. Surfaced by
// connection_pool's `new(size:1) { }.with` smoke run via eval.
#[test] fn eval_kwargs() { run_diff("eval_kwargs"); }

// Module#singleton_class? — true only for eigenclasses. Surfaced by
// sorbet-runtime's method-hook installer (`mod.singleton_class?`).
#[test] fn singleton_class_predicate() { run_diff("singleton_class_predicate"); }

// `require "foo/1.0"` appends `.rb` (→ foo/1.0.rb) rather than treating
// the trailing `.0` as an extension. Surfaced by the rss gem's
// `require "rss/1.0"` / "rss/2.0".
#[test] fn require_dotted_path() { run_diff("require_dotted_path"); }

// `yield(*v, **h)` — a yield mixing a positional splat with a keyword
// double-splat. The trailing KeywordHashNode must route through
// tr_kwhash in the splat-assembly path. Surfaced by the pp gem
// (`yield(*v, **kwsplat)`, pp.rb:277).
#[test] fn yield_splat_kwsplat() { run_diff("yield_splat_kwsplat"); }

// A `class Base < Struct` with its own `[]` override (calling super) is
// honored + super-reachable by member-structs built from it. Surfaced
// by faraday's Options#[] memoization.
#[test] fn struct_subclass_index_override() { run_diff("struct_subclass_index_override"); }

// `class << self; attr_reader(*NAMES); end` — attr_* with a runtime
// splat of names, desugared to a singleton-class send. Surfaced by
// mail's multibyte/unicode.rb.
#[test] fn class_self_attr_splat() { run_diff("class_self_attr_splat"); }

// Bare `tap` / `yield_self` (implicit self) inside an instance method
// dispatches on self. Surfaced by mail's CommonField#parse (`tap(&:element)`).
#[test] fn bare_tap_then() { run_diff("bare_tap_then"); }

// `defined?(super)` — "super" when the enclosing method has a super in
// the chain, else nil. Surfaced by sorbet's `if defined?(super); super`.
#[test] fn defined_super() { run_diff("defined_super"); }

// Nested / parenthesized multiple-assignment targets — `(a, b), c = …`,
// `a, (b, *c) = …`, deep nesting. Surfaced by parser/current's lexer.
#[test] fn nested_destructure() { run_diff("nested_destructure"); }

// `defined?(yield)` — "yield" when the enclosing method has a block,
// else nil. Surfaced by sequel's `if defined?(yield); return yield(db)`.
#[test] fn defined_yield() { run_diff("defined_yield"); }

// `redo` keyword — re-run the loop iteration / block body. Covers
// while / until / loop-do / each-block / innermost-loop binding.
// Surfaced by rss's `loop do … redo … end`.
#[test] fn redo_keyword() { run_diff("redo_keyword"); }

// Constant resolution through an included module's own table, and a
// bare `Head::Rest` whose head lives in an outer lexical scope.
// Surfaced by rexml (`Entity::NAME` via `include XMLTokens`).
#[test] fn const_via_ancestors_and_lexical() { run_diff("const_via_ancestors_and_lexical"); }

// `private :m` / `public :m` (with name args) inside a `class << X`
// body set X's singleton-method visibility. Surfaced by diff-lcs.
#[test] fn class_lt_lt_private() { run_diff("class_lt_lt_private"); }

// Constant assignment in a `class << <const>` body (referenced bare by
// the body's singleton methods) routes to the real eigenclass-body
// path. Surfaced by diff-lcs's `class << Diff::LCS; PATCH_MAP = {…}`.
#[test] fn class_lt_lt_const_body() { run_diff("class_lt_lt_const_body"); }

// Introspecting an eigenclass shell (`Klass.singleton_class
// .instance_method(:m)` / `.instance_methods(false)`) sees class-level
// singleton methods. Surfaced by sorbet's run_sig reflection.
#[test] fn eigenclass_introspection() { run_diff("eigenclass_introspection"); }

// `Object.instance_method(:method)` resolves the Kernel builtin, and an
// Object/BasicObject/Kernel-rooted UnboundMethod binds to ANY receiver.
// Surfaced by sorbet's `Object.instance_method(:method).bind_call(...)`.
#[test] fn object_method_bind() { run_diff("object_method_bind"); }

// Block auto-splat of a single Array into a block with fixed params +
// a rest (`|a, *b|`). Surfaced by rss's `.each { |name, occurs, type,
// *args| }` over arrays-of-rows.
#[test] fn block_autosplat_rest() { run_diff("block_autosplat_rest"); }

// `require "English"` aliases $MATCH/$PREMATCH/$POSTMATCH/etc. to the
// punctuation match globals. Surfaced by rss building method names
// from `$POSTMATCH`.
#[test] fn english_match_globals() { run_diff("english_match_globals"); }

// `Mod.module_eval(string)` runs with the receiver as cref, so bare
// constants resolve through the receiver's namespace. Surfaced by rss.
#[test] fn module_eval_const_scope() { run_diff("module_eval_const_scope"); }

// `Module#constants` lists nested classes/modules defined via the
// compact `class M::Foo` form (not just `module M; class Foo`).
// Surfaced by regexp_parser's `class Regexp::Syntax::V1_8_6` versions.
#[test] fn constants_compact_class() { run_diff("constants_compact_class"); }

// `Regexp#encoding` — US-ASCII for all-ASCII source, else UTF-8.
// Surfaced by regexp_parser's scanner extract_encoding.
#[test] fn regexp_encoding() { run_diff("regexp_encoding"); }
#[test] fn integer_ord() { run_diff("integer_ord"); }

// `Module#dup` shallow-copies into a fresh anonymous module. Surfaced by
// the `inclusive` gem's `ModuleWithPackages.dup` (bridgetown packages DSL).
#[test] fn module_dup() { run_diff("module_dup"); }

// A refinement on a class applies to its subclasses too. Surfaced by
// bridgetown-foundation's `refine ::Hash` deep_dup called on a
// `HashWithDotAccess::Hash`.
#[test] fn refinement_applies_to_subclass() { run_diff("refinement_applies_to_subclass"); }

// A Hash/Array subclass dispatches unknown methods to its class's
// `method_missing`. Surfaced by `HashWithDotAccess::Hash` dot-access
// (Bridgetown's `Configuration` keys-as-methods).
#[test] fn hash_subclass_method_missing() { run_diff("hash_subclass_method_missing"); }

// `Thread.attr_accessor :x` + `Thread.current.x` round-trips (class-level
// accessor in the single-thread model). Surfaced by
// bridgetown-core/current.rb's thread-state store.
#[test] fn thread_attr_accessor() { run_diff("thread_attr_accessor"); }

// `Set#filter_map`. Surfaced by bridgetown-core's
// `configure_component_paths`.
#[test] fn set_filter_map() { run_diff("set_filter_map"); }

// `File.path(obj)` — path-string of a path-like object. Surfaced by the
// vendored fileutils' `rm_f` during Bridgetown's LoadersManager.
#[test] fn file_path_classmethod() { run_diff("file_path_classmethod"); }

// Bare `super` inside a `def m(...)` forwards the anonymous rest/kwrest/
// block like `super(...)` (not slot-dumping the rest array as one
// positional). Surfaced by signalize's `def self.signal_accessor(...)`.
#[test] fn bare_super_dotdotdot() { run_diff("bare_super_dotdotdot"); }

// Ruby 3.1+ `Class#subclasses` (immediate subclasses). Surfaced by
// bridgetown-foundation's `Class#descendants` in Site.new.
#[test] fn class_subclasses() { run_diff("class_subclasses"); }

// `Pathname#join`. Surfaced by bridgetown-core/collection.rb#relative_path.
#[test] fn pathname_join() { run_diff("pathname_join"); }
#[cfg(feature = "stdlib")]
#[test] fn pathname_realpath() { run_diff("pathname_realpath"); }
#[cfg(feature = "stdlib")]
#[test] fn require_force_reload() { run_diff("require_force_reload"); }
#[cfg(feature = "stdlib")]
#[test] fn require_loaded_features_expand_path() { run_diff("require_loaded_features_expand_path"); }
#[cfg(feature = "stdlib")]
#[test] fn file_symlink_readlink() { run_diff("file_symlink_readlink"); }
#[cfg(feature = "stdlib")]
#[test] fn to_set_without_require() { run_diff("to_set_without_require"); }

// Kernel#Pathname() + Pathname#{expand_path, basename(suffix), fnmatch?}
// — the vendored Pathname surface Bridgetown's Site read path uses.
#[test] fn pathname_read_path_methods() { run_diff("pathname_read_path_methods"); }

// `Numeric#nonzero?` (self / nil). Surfaced by signalize's `_dispose`.
#[test] fn numeric_nonzero() { run_diff("numeric_nonzero"); }

// `Integer#to_int` (identity) + its respond_to? whitelist entry (and
// nonzero?'s). Surfaced by tilt's `process_arg` (`arg.respond_to?(:to_int)`).
#[test] fn integer_to_int_respond() { run_diff("integer_to_int_respond"); }

// Bare `freeze` (implicit self) inside a method freezes self. Surfaced
// by erubi's `Engine#initialize`.
#[test] fn bare_freeze_self() { run_diff("bare_freeze_self"); }

// `String#encode!` (in-place encode via replace). Surfaced by
// bridgetown-core's `ERBView#initialize`.
#[test] fn string_encode_bang() { run_diff("string_encode_bang"); }

// A large (>256 char) lookaround/possessive pattern whose fancy-regex
// build is DEFERRED to first use must construct + match identically to
// an eager build. Locks in the lazy-fancy compilation path.
#[test] fn regex_large_lookaround_lazy() { run_diff("regex_large_lookaround_lazy"); }

// Instance variables on a String value (side-table storage). Surfaced
// by serbea's `String#html_safe` on the Bridgetown render path.
#[test] fn string_instance_variables() { run_diff("string_instance_variables"); }

// Ruby 3.2+ Struct keyword init (`S.new(a: 1, b: 2)` on a default
// Struct). Surfaced by bridgetown's front-matter `Result.new(content:,
// front_matter:, line_count:)`.
#[test] fn struct_keyword_init() { run_diff("struct_keyword_init"); }

// `Pathname#each_filename`. Surfaced by bridgetown's resource write path.
#[test] fn pathname_each_filename() { run_diff("pathname_each_filename"); }

// `File.utime(atime, mtime, *paths)` (Integer/Time args). Surfaced by
// bridgetown's `StaticFile#write`.
#[test] fn file_utime() { run_diff("file_utime"); }

// A lexically-scoped autoloaded constant wins over a same-named toplevel
// constant. Surfaced by bridgetown's `register YAML` inside
// `module …FrontMatter::Loaders` (binds `Loaders::YAML`, not `::YAML`).
#[test] fn lexical_autoload_over_toplevel() { run_diff("lexical_autoload_over_toplevel"); }
#[test] fn const_added_scoped_class() { run_diff("const_added_scoped_class"); }
#[test] fn const_added_assignment() { run_diff("const_added_assignment"); }
#[test] fn require_to_path() { run_diff("require_to_path"); }
#[test] fn private_constant() { run_diff("private_constant"); }
#[test] fn constants_includes_autoload() { run_diff("constants_includes_autoload"); }
#[test] fn marshal_load_autoload() { run_diff("marshal_load_autoload"); }
#[test] fn autoload_inception_ignored() { run_diff("autoload_inception_ignored"); }
#[test] fn require_consumes_autoload() { run_diff("require_consumes_autoload"); }
#[test] fn autoload_nearer_ancestor_wins() { run_diff("autoload_nearer_ancestor_wins"); }
#[test] fn qualified_write_fires_owner_autoload() { run_diff("qualified_write_fires_owner_autoload"); }
#[test] fn random_mt19937_exact() { run_diff("random_mt19937_exact"); }
#[test] fn remove_const_clears_source_location() { run_diff("remove_const_clears_source_location"); }
#[test] fn singleton_method_yield_capture_gc() { run_diff("singleton_method_yield_capture_gc"); }
#[test] fn const_defined_autoload() { run_diff("const_defined_autoload"); }
#[test] fn real_mod_name_bind() { run_diff("real_mod_name_bind"); }
