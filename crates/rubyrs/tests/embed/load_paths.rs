//! `Config::load_paths` (#225) tests. The embedder-supplied
//! `$LOAD_PATH` seed: a typed Config field that lets a host
//! pre-populate the require resolver's search path declaratively
//! at Runtime construction, instead of injecting a synthetic
//! `$LOAD_PATH.unshift(...)` as the first eval.
//!
//! Coverage matches the acceptance criteria on issue #225:
//!   1. `load_paths: Some([p])` → scripts see `$LOAD_PATH == [p]`
//!      at the first eval.
//!   2. Script-side `$LOAD_PATH.unshift(...)` still adds on top.
//!   3. `require "name"` resolves against a seeded path.
//!   4. `Config::default().load_paths == None`, no behavioural
//!      change for current users.
//!   5. The seed is construction-time-only — `apply_config` later
//!      does NOT re-seed (would clobber script-side unshifts).
//!   6. Multiple paths preserve insertion order.

use std::path::PathBuf;

use rubyrs::{Config, Runtime, Value};

/// Inline tempdir for the `require`-from-seed test. Mirrors the
/// pattern in `filesystem_sandbox::alloc_tempdir` (RAII via Drop)
/// but lives here to keep the load-paths module standalone — no
/// cross-module `use` from a sibling test file. Tag is the
/// per-test slug; cleanup runs on every exit path including
/// panic-unwind.
struct TempDirGuard {
    path: PathBuf,
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn alloc_tempdir(tag: &str) -> (TempDirGuard, PathBuf) {
    let raw = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("rubyrs-load-paths-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&raw).expect("mkdir tempdir");
    let guard = TempDirGuard { path: raw.clone() };
    let dir = std::fs::canonicalize(&raw).expect("canonicalize tempdir");
    (guard, dir)
}

#[test]
fn default_config_has_none_load_paths() {
    // Regression guard for acceptance criterion #4: hosts that
    // never touched `load_paths` get the pre-PR behaviour. Asserts
    // both the Config field default AND the script-visible
    // $LOAD_PATH (empty Array, lazily allocated on first read).
    let cfg = Config::default();
    assert!(cfg.load_paths.is_none(), "Default must opt out of seeding");
    let mut rt = Runtime::new();
    let v = rt.eval(r#"$LOAD_PATH"#, "test.rb").unwrap();
    // Empty Array — same shape as pre-PR (`$LOAD_PATH` was lazy-
    // initialised to an empty Vec on first read).
    let arr = match &v {
        Value::Array(id) => rt.resolve_array(&Value::Array(*id))
            .expect("array slot should be readable"),
        other => panic!("expected Array, got {other:?}"),
    };
    assert!(arr.is_empty(), "default $LOAD_PATH must be empty; got {arr:?}");
}

#[test]
fn load_paths_seed_visible_at_first_eval() {
    // Acceptance criterion #1: the host's seed shows up as
    // `$LOAD_PATH` in script-visible state at the very first eval,
    // before any user code runs. This is the load-bearing
    // contract — without it, the entire purpose of the field
    // (declarative seed instead of synthetic unshift) collapses.
    let mut rt = Runtime::with_config(Config {
        load_paths: Some(vec![
            PathBuf::from("/usr/share/myapp/lib"),
            PathBuf::from("/usr/share/myapp/vendor"),
        ]),
        ..Default::default()
    });
    let v = rt.eval(r#"$LOAD_PATH"#, "test.rb").unwrap();
    let arr = match &v {
        Value::Array(id) => rt.resolve_array(&Value::Array(*id))
            .expect("array slot readable"),
        other => panic!("expected Array, got {other:?}"),
    };
    // Order matches Vec insertion order — first element of the
    // Vec is at index 0 of $LOAD_PATH, matching CRuby's
    // `unshift`-in-reverse intuition (earlier in Vec = earlier
    // in search).
    assert_eq!(arr.len(), 2);
    let s0 = match &arr[0] {
        Value::Str(s) => s.to_string_lossy(),
        other => panic!("expected Str at [0], got {other:?}"),
    };
    let s1 = match &arr[1] {
        Value::Str(s) => s.to_string_lossy(),
        other => panic!("expected Str at [1], got {other:?}"),
    };
    assert_eq!(s0, "/usr/share/myapp/lib");
    assert_eq!(s1, "/usr/share/myapp/vendor");
}

#[test]
fn script_can_unshift_on_top_of_seed() {
    // Acceptance criterion #2: script-side `$LOAD_PATH.unshift`
    // adds on top of the seeded entries — the seed is not
    // frozen, just pre-populated. Without this, hosts couldn't
    // mix Config seeding with runtime `unshift` calls (rare but
    // legitimate: a host seeds the stdlib path, the user adds a
    // project-specific path at boot).
    let mut rt = Runtime::with_config(Config {
        load_paths: Some(vec![PathBuf::from("/seed")]),
        ..Default::default()
    });
    let v = rt
        .eval(
            r#"$LOAD_PATH.unshift("/runtime"); $LOAD_PATH"#,
            "test.rb",
        )
        .unwrap();
    let arr = match &v {
        Value::Array(id) => rt.resolve_array(&Value::Array(*id))
            .expect("array slot readable"),
        other => panic!("expected Array, got {other:?}"),
    };
    assert_eq!(arr.len(), 2);
    // `unshift("/runtime")` puts it at index 0, pushing the
    // seed to index 1. Standard Ruby semantics.
    let s0 = match &arr[0] {
        Value::Str(s) => s.to_string_lossy(),
        _ => panic!("expected Str"),
    };
    let s1 = match &arr[1] {
        Value::Str(s) => s.to_string_lossy(),
        _ => panic!("expected Str"),
    };
    assert_eq!(s0, "/runtime");
    assert_eq!(s1, "/seed");
}

#[test]
#[cfg(not(target_os = "wasi"))]
fn require_resolves_against_seeded_load_path() {
    // Acceptance criterion #3: `require "name"` walks the seeded
    // entries during the candidate-resolution phase. The seeded
    // dir contains `name.rb`; without `Config::load_paths`, the
    // require would fail to find it (caller-dir + parent-dir
    // candidates don't include the tempdir).
    let (_guard, dir) = alloc_tempdir("require-resolve");
    let lib_file = dir.join("seeded_helper.rb");
    std::fs::write(&lib_file, "SEEDED_HELPER_LOADED = true").expect("write helper");

    let mut rt = Runtime::with_config(Config {
        // Both fields needed for the canonical rubund-style shape:
        // allow_filesystem_io true (require needs to read), and
        // load_paths populated so the require resolver finds the
        // .rb file outside the caller-source dir.
        allow_filesystem_io: true,
        load_paths: Some(vec![dir.clone()]),
        ..Default::default()
    });
    let v = rt
        .eval(
            r#"require "seeded_helper"; SEEDED_HELPER_LOADED"#,
            "caller.rb",
        )
        .unwrap();
    assert!(
        matches!(v, Value::Bool(true)),
        "require should resolve via seeded $LOAD_PATH and load the file",
    );
}

#[test]
fn apply_config_does_not_reseed_load_paths() {
    // Acceptance criterion #5: `load_paths` is construction-time-
    // only. A mid-life `apply_config` re-seed would clobber any
    // script-side `unshift` made between construction and the
    // reconfig — almost never what a host wants.
    //
    // Setup: construct with seed `/seed1`. Script unshifts
    // `/runtime`. Then `apply_config` runs with a DIFFERENT
    // load_paths value (`/seed2`). Expected post-reconfig
    // $LOAD_PATH: the original `/runtime` + `/seed1` survive
    // unchanged; `/seed2` is silently ignored.
    let mut rt = Runtime::with_config(Config {
        load_paths: Some(vec![PathBuf::from("/seed1")]),
        ..Default::default()
    });
    rt.eval(r#"$LOAD_PATH.unshift("/runtime")"#, "test.rb").unwrap();
    rt.apply_config(Config {
        load_paths: Some(vec![PathBuf::from("/seed2")]),
        ..Default::default()
    });
    let v = rt.eval(r#"$LOAD_PATH"#, "test.rb").unwrap();
    let arr = match &v {
        Value::Array(id) => rt.resolve_array(&Value::Array(*id))
            .expect("array slot readable"),
        other => panic!("expected Array, got {other:?}"),
    };
    // Both pre-reconfig entries survive; the new seed is dropped.
    let strs: Vec<String> = arr.iter().map(|v| match v {
        Value::Str(s) => s.to_string_lossy(),
        _ => panic!("expected Str"),
    }).collect();
    assert_eq!(strs, vec!["/runtime".to_string(), "/seed1".to_string()],
        "apply_config must not re-seed (would clobber script-side unshifts); \
         /seed2 should have been ignored");
}

#[test]
fn empty_load_paths_seed_is_noop() {
    // Edge case: `Some(vec![])` — explicitly empty seed. The
    // `seed_load_path` helper short-circuits on `paths.is_empty()`
    // so the seeding step itself is a true no-op (doesn't even
    // call `ensure_load_path`, doesn't materialise the Array).
    // The Array does still get allocated LATER when the test
    // below evaluates `$LOAD_PATH` for the first time — that's
    // the existing lazy-init path, unrelated to seeding. What
    // the test locks in: no seed → no panic, no entries.
    let mut rt = Runtime::with_config(Config {
        load_paths: Some(vec![]),
        ..Default::default()
    });
    let v = rt.eval(r#"$LOAD_PATH"#, "test.rb").unwrap();
    let arr = match &v {
        Value::Array(id) => rt.resolve_array(&Value::Array(*id))
            .expect("array readable"),
        other => panic!("expected Array, got {other:?}"),
    };
    assert!(arr.is_empty(), "Some(vec![]) should produce empty $LOAD_PATH");
}

#[test]
fn load_paths_seed_survives_reset() {
    // The seed lands in the post-preamble snapshot (because
    // `with_config` seeds BEFORE `post_preamble.capture`), so the
    // ObjId is restored on `Runtime::reset` and the heap slot
    // holding the seeded entries survives the heap truncation
    // (the slot's index is BELOW snapshot.heap_slot_count).
    //
    // Note: `reset` is *slot-aware* but not *contents-aware* — it
    // doesn't roll back the Vec<Value> stored in the heap slot.
    // So script-side `$LOAD_PATH.unshift(...)` calls between
    // `with_config` and `reset` PERSIST through reset (existing
    // behaviour, not load_paths-specific). This test asserts only
    // the load_paths-specific contract: the SEED is still there
    // afterward. A future enhancement that rolls back Array
    // contents on reset would tighten this — the assertion below
    // accommodates either policy by using `contains` rather than
    // exact equality.
    let mut rt = Runtime::with_config(Config {
        load_paths: Some(vec![PathBuf::from("/persistent-seed")]),
        ..Default::default()
    });
    rt.reset();
    let v = rt.eval(r#"$LOAD_PATH"#, "test.rb").unwrap();
    let arr = match &v {
        Value::Array(id) => rt.resolve_array(&Value::Array(*id))
            .expect("array readable"),
        other => panic!("expected Array, got {other:?}"),
    };
    let strs: Vec<String> = arr.iter().map(|v| match v {
        Value::Str(s) => s.to_string_lossy(),
        _ => panic!("expected Str"),
    }).collect();
    assert!(
        strs.iter().any(|s| s == "/persistent-seed"),
        "reset should preserve the load_paths seed; got {strs:?}",
    );
}
