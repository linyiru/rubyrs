//! Corner cases that don't fit any of the other topical
//! sub-modules. Grouped here rather than scattered through
//! the main `embed.rs` smoke surface to keep that root file
//! focused on the canonical embedding-API tests.
//!
//! Three loose clusters:
//!
//!   1. **`Runtime::resolve_*` helpers** — `resolve_array` /
//!      `resolve_hash` / `resolve_sym` unpack a `Value` into
//!      Rust-side form (Vec / Vec<(k,v)> / String).
//!      Edge-case behaviour: non-matching variant returns
//!      `None`; round-trip stability for Symbols.
//!   2. **Real-world DSL host integration** —
//!      `gemfile_dsl_real_hosting_end_to_end` runs the full
//!      bundler `Gemfile` shape through a host-side DSL
//!      interpreter built on `register_fn_v2`. End-to-end
//!      proves the embedding API is enough to host a
//!      non-trivial Ruby DSL.
//!   3. **Pinned divergences from CRuby** — corner cases
//!      where rubyrs behaviour deliberately or pragmatically
//!      differs from CRuby, kept under a `*_today` /
//!      `*_returns_nil` suffix to flag them as "this is the
//!      current contract, change requires updating both the
//!      code and this test":
//!      - `range_max_with_i64_min_exclusive_returns_nil`
//!      - `range_size_with_i64_max_width_returns_zero`
//!      - `array_first_last_non_int_n_raises_no_method_error_today`
//!      - `range_first_last_non_int_n_raises_no_method_error_today`
//!      - `interpolated_regex_invalid_pattern_returns_syntax_error_trap`
//!        (CRuby would raise RegexpError, not SyntaxError)

use rubyrs::{Runtime, RubyError, Trap, Value};

use super::SharedBuf;

#[test]
fn resolve_array_unpacks_elements() {
    let mut rt = Runtime::new();
    let val = rt.eval("[10, 20, 30]", "t.rb").unwrap();
    let elems = rt.resolve_array(&val).expect("should be an Array");
    assert_eq!(elems.len(), 3);
    assert!(matches!(elems[0], Value::Int(10)));
    assert!(matches!(elems[1], Value::Int(20)));
    assert!(matches!(elems[2], Value::Int(30)));
}

#[test]
fn resolve_array_returns_none_for_non_array() {
    let rt = Runtime::new();
    assert!(rt.resolve_array(&Value::Int(42)).is_none());
}

#[test]
fn resolve_hash_unpacks_pairs() {
    let mut rt = Runtime::new();
    let val = rt.eval(r#"{ "a" => 1, "b" => 2 }"#, "t.rb").unwrap();
    let pairs = rt.resolve_hash(&val).expect("should be a Hash");
    assert_eq!(pairs.len(), 2);
    assert!(matches!(&pairs[0].0, Value::Str(s) if s.to_string_lossy() == "a"));
    assert!(matches!(&pairs[0].1, Value::Int(1)));
    assert!(matches!(&pairs[1].0, Value::Str(s) if s.to_string_lossy() == "b"));
    assert!(matches!(&pairs[1].1, Value::Int(2)));
}

#[test]
fn resolve_hash_returns_none_for_non_hash() {
    let rt = Runtime::new();
    assert!(rt.resolve_hash(&Value::Nil).is_none());
}

#[test]
fn resolve_sym_roundtrips_symbol() {
    let mut rt = Runtime::new();
    let val = rt.eval(":hello", "t.rb").unwrap();
    if let Value::Sym(id) = val {
        assert_eq!(rt.resolve_sym(id), "hello");
    } else {
        panic!("expected Value::Sym, got {:?}", val);
    }
}

#[test]
fn gemfile_dsl_real_hosting_end_to_end() {
    // Locks in the `examples/gemfile/` demo at integration-test
    // shape: prelude + unmodified Gemfile + the same Rust host
    // surface, all driven through the public Runtime API.
    // Asserts the gem-count + group bucketing the demo produces
    // so any regression in (kwargs / splat receive / group block
    // yielding / `if RUBY_VERSION` conditional / `**opts` Hash
    // unpacking in the prelude) shows up here, not just when
    // someone happens to re-run the example binary.
    use std::cell::RefCell;
    use std::rc::Rc;

    // Mirror the example's GemfileState shape — small enough to
    // dup here and keeps the test self-contained. Named fields
    // (rather than a positional tuple) so the assertions below
    // read as `puma.require_kw` not `puma.3` — much harder to
    // mis-order when the schema grows.
    #[derive(Default)]
    struct Gem {
        name: String,
        reqs: Vec<String>,
        groups: Vec<String>,
        require_kw: String,
        platforms_kw: String,
        platforms_scope: Vec<String>,
        source_override: Option<(String, String)>,
    }
    #[derive(Default)]
    struct State {
        source: Option<String>,
        ruby_version: Option<String>,
        gems: Vec<Gem>,
        group_stack: Vec<String>,
        platforms_stack: Vec<String>,
        // Unified source-override stack — matches the example's
        // shape so `git` / `path` precedence is push-order, not
        // type-priority. See `examples/gemfile.rs::GemfileState`.
        source_stack: Vec<(String, String)>,
    }
    let state = Rc::new(RefCell::new(State::default()));
    let mut rt = Runtime::new();

    fn s(v: &Value) -> String {
        if let Value::Str(rs) = v { rs.to_string_lossy() } else { String::new() }
    }

    {
        let st = state.clone();
        rt.register_fn("__gemfile_source", move |args| {
            if let [u] = args { st.borrow_mut().source = Some(s(u)); }
            Ok(Value::Nil)
        });
    }
    {
        let st = state.clone();
        rt.register_fn("__gemfile_ruby", move |args| {
            if let [v] = args { st.borrow_mut().ruby_version = Some(s(v)); }
            Ok(Value::Nil)
        });
    }
    // v2 form — mirrors examples/gemfile.rs::__gemfile_gem_v2.
    // Fail-fast shape validation: matches the demo's pattern and
    // the earlier register_fn_v2_reads_* unit tests. A regression
    // in the prelude (sending the wrong shape) surfaces as an
    // ArgumentError here, not as a silent partial GemfileState
    // that fails 200 lines later in `.gems.len() != 18`.
    {
        let st = state.clone();
        rt.register_fn_v2("__gemfile_gem_v2", move |ctx, args| {
            let [name, requirements, opts] = args else {
                return Err(Trap {
                    err: RubyError::ArgumentError {
                        msg: format!("__gemfile_gem_v2: expected 3 args, got {}", args.len()),
                    },
                    backtrace: vec![],
                });
            };
            let name = if let Value::Str(rs) = name {
                rs.to_string_lossy()
            } else {
                return Err(Trap {
                    err: RubyError::ArgumentError { msg: "name must be a String".into() },
                    backtrace: vec![],
                });
            };
            let reqs_slice = ctx.resolve_array(requirements).ok_or_else(|| Trap {
                err: RubyError::ArgumentError { msg: "requirements must be an Array".into() },
                backtrace: vec![],
            })?;
            let opts_slice = ctx.resolve_hash(opts).ok_or_else(|| Trap {
                err: RubyError::ArgumentError { msg: "opts must be a Hash".into() },
                backtrace: vec![],
            })?;

            let reqs_vec: Vec<String> = reqs_slice.iter()
                .map(|v| if let Value::Str(rs) = v {
                    Ok(rs.to_string_lossy())
                } else {
                    Err(Trap {
                        err: RubyError::ArgumentError {
                            msg: "requirements element must be a String".into(),
                        },
                        backtrace: vec![],
                    })
                })
                .collect::<Result<_, _>>()?;
            // Bundler kwargs Hash: Symbol keys, mixed values (Bool /
            // Sym / String). Mirrors examples/gemfile.rs.
            let mut require_kw = String::new();
            let mut platforms_kw = String::new();
            for (k, v) in opts_slice {
                let key = ctx.resolve_sym(k).ok_or_else(|| Trap {
                    err: RubyError::ArgumentError {
                        msg: "opts keys must be Symbols".into(),
                    },
                    backtrace: vec![],
                })?;
                let vs = match v {
                    Value::Bool(b) => if *b { "true".to_string() } else { "false".to_string() },
                    Value::Str(rs) => rs.to_string_lossy(),
                    // The outer match already filtered on `Value::Sym`,
                    // so `resolve_sym` is guaranteed to return Some.
                    // `expect` rather than `unwrap_or("")` so a future
                    // interner-contract regression surfaces loudly.
                    Value::Sym(_) => ctx.resolve_sym(v)
                        .expect("resolve_sym on Value::Sym arm must return Some")
                        .to_string(),
                    _ => return Err(Trap {
                        err: RubyError::ArgumentError {
                            msg: format!("opts[{key}] must be a Bool, Symbol, or String"),
                        },
                        backtrace: vec![],
                    }),
                };
                match key {
                    "require"   => require_kw   = vs,
                    "platforms" => platforms_kw = vs,
                    _ => {}
                }
            }

            let mut sm = st.borrow_mut();
            let groups: Vec<String> = sm.group_stack.last()
                .map(|s| s.split(',').filter(|x| !x.is_empty()).map(String::from).collect())
                .unwrap_or_default();
            let platforms_scope: Vec<String> = sm.platforms_stack.last()
                .map(|s| s.split(',').filter(|x| !x.is_empty()).map(String::from).collect())
                .unwrap_or_default();
            let source_override = sm.source_stack.last().cloned();
            sm.gems.push(Gem {
                name,
                reqs: reqs_vec,
                groups,
                require_kw,
                platforms_kw,
                platforms_scope,
                source_override,
            });
            Ok(Value::Nil)
        });
    }
    {
        let st = state.clone();
        rt.register_fn("__gemfile_push_groups", move |args| {
            if let [v] = args { st.borrow_mut().group_stack.push(s(v)); }
            Ok(Value::Nil)
        });
    }
    {
        let st = state.clone();
        rt.register_fn("__gemfile_pop_groups", move |_args| {
            st.borrow_mut().group_stack.pop();
            Ok(Value::Nil)
        });
    }
    // Real push/pop wiring for platforms / git / path so a
    // regression in those scope blocks (block-yield ordering,
    // ensure-pop pairing, source-precedence) actually fails
    // the test instead of silently no-op'ing.
    {
        let st = state.clone();
        rt.register_fn("__gemfile_push_platforms", move |args| {
            if let [v] = args { st.borrow_mut().platforms_stack.push(s(v)); }
            Ok(Value::Nil)
        });
    }
    {
        let st = state.clone();
        rt.register_fn("__gemfile_pop_platforms", move |_args| {
            st.borrow_mut().platforms_stack.pop();
            Ok(Value::Nil)
        });
    }
    for (push_name, pop_name, kind) in [
        ("__gemfile_push_git",  "__gemfile_pop_git",  "git"),
        ("__gemfile_push_path", "__gemfile_pop_path", "path"),
    ] {
        let st = state.clone();
        let kind_s: String = kind.into();
        rt.register_fn(push_name, move |args| {
            if let [v] = args {
                st.borrow_mut().source_stack.push((kind_s.clone(), s(v)));
            }
            Ok(Value::Nil)
        });
        let st = state.clone();
        rt.register_fn(pop_name, move |_args| {
            st.borrow_mut().source_stack.pop();
            Ok(Value::Nil)
        });
    }

    // Read the actual prelude + Gemfile from the repo. That's
    // the point: the demo and the test exercise the same files.
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("examples/gemfile");
    let prelude_src = std::fs::read_to_string(base.join("dsl_prelude.rb"))
        .expect("dsl_prelude.rb missing — examples/gemfile/ removed?");
    let gemfile_src = std::fs::read_to_string(base.join("Gemfile"))
        .expect("Gemfile missing — examples/gemfile/ removed?");

    rt.eval(&prelude_src, "dsl_prelude.rb").expect("prelude eval");
    rt.eval(&gemfile_src, "Gemfile").expect("Gemfile eval");

    let st = state.borrow();
    assert_eq!(st.source.as_deref(), Some("https://rubygems.org"));
    assert_eq!(st.ruby_version.as_deref(), Some("3.4.0"));
    // 15 from the original list + rb-readline + forked-gem +
    // vendored-gem = 18. The negative `if RUBY_VERSION >=
    // "99.0.0"` branch must NOT contribute `future-only-gem`.
    assert_eq!(st.gems.len(), 18,
        "expected 18 gems from examples/gemfile/Gemfile, got {}",
        st.gems.len());

    let find = |n: &str| st.gems.iter().find(|g| g.name == n)
        .unwrap_or_else(|| panic!("{n} missing"));

    // Spot-check the splat-receive case: rack should have 2
    // version constraints, not 1.
    assert_eq!(find("rack").reqs, vec![">= 3.0", "< 4.0"]);

    // Spot-check the multi-group block: rspec-rails should be
    // tagged with BOTH `:development` and `:test`.
    assert_eq!(find("rspec-rails").groups, vec!["development", "test"]);

    // Conditional truthy branch: with prelude setting
    // RUBY_VERSION = "3.4.0", `csv` (guarded by >= "3.4.0")
    // should be present.
    assert!(st.gems.iter().any(|g| g.name == "csv"),
        "csv should be present when RUBY_VERSION >= 3.4.0");
    // Conditional falsy branch: `future-only-gem` is guarded by
    // `if RUBY_VERSION >= "99.0.0"`. If String `>=` inverted or
    // `if` polarity flipped, this gem would sneak in.
    assert!(!st.gems.iter().any(|g| g.name == "future-only-gem"),
        "future-only-gem must NOT appear under RUBY_VERSION 3.4.0");

    // `**kwargs` Hash round-trip into our named fields. A
    // regression in Hash receive / Symbol-key / `.to_s` would
    // blank these out.
    let puma = find("puma");
    assert_eq!(puma.require_kw, "false", "puma's require: false should round-trip");
    assert_eq!(puma.platforms_kw, "", "puma has no platforms: kwarg");

    let sidekiq = find("sidekiq");
    assert_eq!(sidekiq.require_kw, "sidekiq", "sidekiq's require: 'sidekiq' should round-trip");
    assert_eq!(sidekiq.platforms_kw, "mri", "sidekiq's platforms: :mri should round-trip");

    let pry = find("pry-byebug");
    assert_eq!(pry.require_kw, "pry-byebug");
    assert_eq!(pry.platforms_kw, "mri");

    // Bare gem — no kwargs, both slots empty.
    let rake = find("rake");
    assert_eq!(rake.require_kw, "");
    assert_eq!(rake.platforms_kw, "");

    // `platforms :mri do ... end` block — rb-readline picks up
    // the platforms_scope via the push/pop wiring above.
    let rb_readline = find("rb-readline");
    assert_eq!(rb_readline.platforms_scope, vec!["mri"],
        "rb-readline should inherit platforms-scope from its block");

    // `git "url" do ... end` block — forked-gem picks up the
    // source override. If git/path used separate stacks with
    // git-then-path precedence this would still work for a
    // bare git block, but nested git/path would mis-tag.
    let forked = find("forked-gem");
    assert_eq!(forked.source_override,
        Some(("git".into(), "https://github.com/example/forked-gem.git".into())),
        "forked-gem should be tagged with its enclosing git source");

    // `path "..." do ... end` block — vendored-gem.
    let vendored = find("vendored-gem");
    assert_eq!(vendored.source_override,
        Some(("path".into(), "vendor/cache".into())),
        "vendored-gem should be tagged with its enclosing path source");

    // None of the gems declared outside a source block should
    // have a stale source_override. If pop_git/pop_path leaked
    // or the unified stack didn't drain, a later gem would
    // pick up an override it shouldn't have.
    assert_eq!(rake.source_override, None,
        "rake (top-level) should have no source override; \
         a non-None here means pop didn't pair with push");
}

#[test]
fn interpolated_regex_invalid_pattern_returns_syntax_error_trap() {
    // PR #99 review coverage: the InterpolatedRegex path documents
    // that invalid runtime-assembled patterns surface as SyntaxError
    // traps at `Op::CompileRegex` (mirroring `LoadRegex`). The
    // existing literal-regex path already returns SyntaxError too,
    // so this is a parity check not a divergence acknowledgement.
    //
    // CRuby raises RegexpError here ("end pattern with unmatched
    // parenthesis"); the class differs from rubyrs's SyntaxError
    // for both literal AND interpolated regex paths, which is why
    // this lives as a host-API test rather than in diff_cruby.
    let mut rt = rubyrs::Runtime::new();
    let err = rt.eval(
        r#"
        bad = "("
        /#{bad}/
        "#,
        "bad_interpolated_regex.rb",
    ).unwrap_err();
    assert!(
        matches!(err.err, rubyrs::RubyError::SyntaxError { .. }),
        "expected SyntaxError trap from invalid interpolated regex, got {:?}",
        err.err,
    );
}

#[cfg(feature = "bignum")]
#[test]
fn range_max_with_i64_min_exclusive_returns_nil() {
    // Regression cover for the /code-review finding: Range#max
    // with an exclusive endpoint computes `ei - 1`. Pre-fix this
    // panicked in debug for ei == i64::MIN; treated as an empty
    // range (Nil) now.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        // (-2**63 ... -2**63).max  — exclusive, endpoint == i64::MIN
        "puts((-9_223_372_036_854_775_808...-9_223_372_036_854_775_808).max.inspect)",
        "range_max_min_excl.rb",
    ).expect("should succeed without panic");
    assert_eq!(buf.snapshot().trim(), "nil");
}

#[cfg(feature = "bignum")]
#[test]
fn range_size_with_i64_max_width_returns_zero() {
    // Pre-fix `ei - bi + 1` panicked in debug when bi == i64::MIN
    // and ei == i64::MAX (width 2^64). Treat overflow as 0.
    let mut rt = rubyrs::Runtime::new();
    let buf = SharedBuf::new();
    rt.set_stdout(Box::new(buf.clone()));
    rt.eval(
        "puts (-9_223_372_036_854_775_808..9_223_372_036_854_775_807).size",
        "range_size_max_width.rb",
    ).expect("should succeed without panic");
    assert_eq!(buf.snapshot().trim(), "0");
}

#[test]
fn array_first_last_non_int_n_raises_no_method_error_today() {
    // Pin the current rubyrs divergence from CRuby on
    // `Array#first(n)` / `Array#last(n)` when `n` isn't an
    // `Int`.
    //
    // CRuby behaviour (2026-05):
    //   - `[1,2,3].first(2.0)` returns `[1, 2]` — Float's
    //     `to_int` coerces to 2.
    //   - `[1,2,3].last(:x)`   raises `TypeError: no implicit
    //     conversion of Symbol into Integer`.
    //
    // rubyrs behaviour: both raise `NoMethodError: undefined
    // method 'first'/'last' for Array` because the match arms
    // in `vm/array.rs` only bind `Value::Int(n)`, so Float /
    // Sym / BigInt / etc. fall past the `(n)` arms to the
    // generic NoMethodError catch-all.
    //
    // This test is NOT a diff_cruby fixture because the
    // divergence would make the harness fail. The point is to
    // make the divergence VISIBLE in tree: a future contributor
    // who fixes Float coercion (or wires `to_int` more
    // generally) will see this test fail, get directed to
    // re-classify Array#first(n) / Array#last(n), and either
    // remove or update this test. Without it, the divergence
    // is invisible — there's no failing breadcrumb when
    // someone partially implements coercion in a way that
    // changes the behaviour here.
    //
    // The `take` / `drop` arms in the same file have the same
    // shape; widening to_int coercion across all Int-taking
    // Array methods would be a separable change.
    // RubyError + Runtime are already in scope from the file-level
    // `use rubyrs::{Config, HostCtx, Runtime, RubyError, Trap, Value};`
    // at the top — no extra import needed.

    fn assert_no_method(src: &str) {
        let mut rt = Runtime::new();
        let err = rt.eval(src, "non_int_n.rb")
            .expect_err("expected error");
        // `RubyError::is()` handles both the direct
        // NoMethodError variant and the Uncaught wrapper that
        // some dispatch paths route through (they both surface
        // as `NoMethodError` to the script).
        assert!(
            err.err.is("NoMethodError"),
            "expected NoMethodError for `{src}`, got {:?}",
            err.err,
        );
    }

    assert_no_method("[1,2,3].first(2.0)");
    assert_no_method("[1,2,3].last(2.0)");
    assert_no_method("[1,2,3].first(:x)");
    assert_no_method("[1,2,3].last(:x)");
    assert_no_method("[1,2,3].first('2')");
    assert_no_method("[1,2,3].last('2')");
}

#[test]
fn range_first_last_non_int_n_raises_no_method_error_today() {
    // Companion to `array_first_last_non_int_n_raises_no_method_error_today`.
    // Pin the current rubyrs divergence from CRuby on
    // `Range#first(n)` / `Range#last(n)` when `n` isn't an
    // `Int`.
    //
    // CRuby behaviour (2026-05):
    //   - `(1..5).first(2.0)` returns `[1, 2]` — Float's
    //     `to_int` coerces to 2.
    //   - `(1..5).last(:x)`   raises `TypeError: no implicit
    //     conversion of Symbol into Integer`.
    //
    // rubyrs behaviour: both raise NoMethodError because the
    // match arms in `vm/range.rs` only bind `Value::Int(n)`.
    // Float / Sym / String fall past the `(n)` arms to the
    // generic NoMethodError catch-all.
    //
    // This test mirrors the Array sibling rather than being a
    // diff_cruby fixture, for the same reason: a diff_cruby
    // fixture would fail the harness because CRuby's output
    // disagrees with rubyrs's. The embed test creates a
    // breadcrumb so a future contributor who wires `to_int`
    // coercion (or adds a Float / BigInt arm) gets a failing
    // test and is forced to re-classify Range#first(n) /
    // Range#last(n) intentionally.

    fn assert_no_method(src: &str) {
        let mut rt = Runtime::new();
        let err = rt.eval(src, "non_int_n.rb")
            .expect_err("expected error");
        assert!(
            err.err.is("NoMethodError"),
            "expected NoMethodError for `{src}`, got {:?}",
            err.err,
        );
    }

    assert_no_method("(1..5).first(2.0)");
    assert_no_method("(1..5).last(2.0)");
    assert_no_method("(1..5).first(:x)");
    assert_no_method("(1..5).last(:x)");
    assert_no_method("(1..5).first('2')");
    assert_no_method("(1..5).last('2')");
    // Endless range too — the endless first(n) arm also only
    // matches Value::Int(n).
    assert_no_method("(1..).first(2.0)");
}


#[test]
fn array_spaceship_self_referential_does_not_overflow_stack() {
    // `value_cmp_v_heap`'s Array×Array recursion would stack-
    // overflow when comparing the same self-referential Array
    // (`a = []; a << a; a <=> a`): a[0] is `a` again, recurse,
    // and the base case never fires. The Array-id short-circuit
    // in `value_cmp_v_heap` catches the direct same-ObjId case
    // and returns Equal. Deeper mutual cycles (`a << b; b << a`)
    // would still overflow — that's a documented gap and is not
    // covered here.
    let mut rt = Runtime::new();
    let v = rt.eval(
        r#"
        a = []
        a << a
        a <=> a
        "#,
        "spaceship_self.rb",
    ).expect("self-spaceship should return Int, not overflow");
    assert!(matches!(v, Value::Int(0)), "expected Int(0), got {:?}", v);
}
