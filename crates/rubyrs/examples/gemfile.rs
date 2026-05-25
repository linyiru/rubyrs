//! Gemfile DSL host — proves rubyrs runs an unmodified, real-shape
//! Gemfile.
//!
//! The Gemfile (`examples/gemfile/Gemfile`) is byte-identical to
//! what you'd check into a Ruby project — no rubyrs-specific
//! tweaks. It exercises every shape a typical Rails-style Gemfile
//! does:
//! - bare `gem "rake"`
//! - version constraint `gem "rails", "~> 8.0.0"`
//! - multi-version splat `gem "rack", ">= 3.0", "< 4.0"`
//! - keyword arguments `gem "puma", require: false`
//! - `group :a, :b do ... end` blocks (multi-symbol)
//! - conditional `if RUBY_VERSION >= "..."` at file scope
//!
//! The Rust host (this file) registers two flavours of host
//! function. `__gemfile_gem_v2` uses the v2 API
//! (`register_fn_v2` paired with `HostCtx`) and reads the splat
//! as a real Array, kwargs as a real Hash, and the Symbol keys
//! inside that Hash via `ctx.resolve_sym`. The HostCtx accessors
//! return **borrows** into the VM heap / interner — zero-copy
//! while the v2 closure runs, no Ruby-side stringification on the
//! input path. (The demo then clones these borrows into
//! `String`s for the `GemDecl` storage that outlives the
//! closure; the host has no other choice unless we want to chain
//! lifetimes through to the global state, which isn't the point
//! of the example.)
//! The scope-stack helpers (`group` / `platforms` / `git` / `path`
//! push/pop) stay on the v1 API — they take a single pre-joined
//! String, which is what the prelude's `ensure`-balanced shims
//! already produce. The Ruby-side prelude
//! (`examples/gemfile/dsl_prelude.rb`) does no translation at all
//! on the `gem` shim: `def gem(name, *requirements, **opts)`
//! becomes a one-line forward to the host. The Gemfile itself
//! never sees the seam.
//!
//! ```text
//!     cargo run --release --example gemfile
//! ```
//!
//! Why this matters: with the metaprog PoC (ADR 0010) closed,
//! rubyrs runs **real** Ruby DSLs — not bespoke ones tailored
//! to the subset. The Brewfile example
//! (`examples/brewfile.rs`) already showed this for the simpler
//! tap/brew/cask shape; this example shows it for the full
//! Gemfile shape with kwargs, version-spec splat, group blocks,
//! conditional file-scope logic, and `ensure`-balanced scope
//! pops. End-to-end runtime stays in the low-millisecond range
//! that's been rubyrs's headline number since the README.
//!
//! ## v1 vs v2 host-fn API in this demo
//!
//! This demo originally relied entirely on v1 (`register_fn`),
//! flattening `*splat` and `**kwargs` to `|`-joined Strings in
//! the prelude before reaching the host. v2 (`register_fn_v2`
//! paired with `HostCtx`) closed that gap: the v2 closure
//! receives the Array and Hash directly and `ctx.resolve_array`
//! / `ctx.resolve_hash` / `ctx.resolve_sym` borrow into the heap
//! and interner. `__gemfile_gem_v2` below uses v2; the
//! scope-stack helpers stay on v1 because their natural input is
//! already a single String.
//!
//! With `resolve_sym` in place (closing the gap PR #40 noted),
//! the prelude does NO translation on the `gem` shim — the kwarg
//! Hash reaches the host with its native Symbol keys and mixed
//! values (Bool / Sym / String), all of which the host handles
//! by direct match.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use rubyrs::{HostCtx, RubyError, Runtime, Trap, Value};

/// Build an `ArgumentError` trap so v2 shape mismatches fail fast
/// rather than silently producing partial state. Demo-quality
/// pattern — embedders copying this should follow the same shape:
/// validate the heap-y arg type, validate every element/entry,
/// fail with a concrete error message.
fn arg_err(msg: impl Into<String>) -> Trap {
    Trap { err: RubyError::ArgumentError { msg: msg.into() }, backtrace: vec![] }
}

/// Stringify a kwarg value for the demo's flat `String` storage.
/// Accepts the three shapes a Bundler-style kwarg actually uses —
/// Bool (`require: false`), Symbol (`platforms: :mri`), String
/// (`require: "pry-byebug"`). Returns `None` for anything else so
/// the caller can `?`-bubble an explicit ArgumentError instead of
/// silently storing "" — the demo's previous shape silently
/// dropped non-String values.
fn value_to_kwarg_string(ctx: &HostCtx, v: &Value) -> Option<String> {
    match v {
        Value::Bool(b) => Some(if *b { "true".into() } else { "false".into() }),
        Value::Str(rs) => Some(rs.to_string_lossy()),
        // The arm already matched `Value::Sym`, so `resolve_sym` is
        // guaranteed to return Some. `expect` rather than `.map` so a
        // future interner-contract regression panics loudly instead
        // of mis-routing through this fn's caller as an ArgumentError
        // ("must be a Bool, Symbol, or String") — which would be a
        // misleading message for a value that IS a Symbol.
        Value::Sym(_) => Some(ctx.resolve_sym(v)
            .expect("resolve_sym on Value::Sym arm must return Some")
            .to_string()),
        _ => None,
    }
}

/// A single gem declaration captured from the Gemfile.
#[derive(Debug, Clone)]
struct GemDecl {
    name: String,
    /// `"~> 8.0.0"`, `">= 3.0"` etc. Empty when the Gemfile just
    /// said `gem "rake"`.
    requirements: Vec<String>,
    /// Groups active at the time of the call. Empty for top-
    /// level gems (effectively `:default`).
    groups: Vec<String>,
    /// `platforms :mri do ... end` scope at call time.
    platforms_scope: Vec<String>,
    /// `git "url" do ... end` or `path "../local" do ... end`
    /// source override active at the call site.
    source_override: Option<(String, String)>,
    /// The `require:` kwarg as a String. Empty if absent.
    require: String,
    /// The `platforms:` kwarg as a String. Empty if absent.
    /// Distinct from the `platforms_scope` block-stack above.
    platforms_kw: String,
}

#[derive(Default, Debug)]
struct GemfileState {
    source: Option<String>,
    ruby_version: Option<String>,
    gems: Vec<GemDecl>,
    // Active scope stacks. Each entry is the comma-joined name
    // list pushed by the corresponding prelude shim, so a
    // single `group :a, :b do ... end` produces one stack
    // entry "a,b" rather than two entries "a" and "b". Gems
    // inside resolve that to a Vec<String> on capture.
    group_stack: Vec<String>,
    platforms_stack: Vec<String>,
    /// Unified source-override stack — `git` and `path` push
    /// onto the same Vec so the most-recently-entered block
    /// always wins. Tracked as `(kind, value)` tuples; the
    /// top entry is the active override. Earlier sketches
    /// kept `git_stack` + `path_stack` separately and checked
    /// git-then-path; for nested `git "url" do path "x" do …`
    /// that returned the outer git rather than the inner path,
    /// silently mis-tagging the inner gem (PR #35 review F1).
    source_stack: Vec<(String, String)>,
}

impl GemfileState {
    /// Flatten the comma-joined top of a scope stack to the
    /// concrete name list. `gem` capture uses this to record
    /// the full list each declaration was tagged with.
    fn active_groups(&self) -> Vec<String> {
        self.group_stack.last()
            .map(|s| s.split(',').filter(|x| !x.is_empty()).map(String::from).collect())
            .unwrap_or_default()
    }
    fn active_platforms(&self) -> Vec<String> {
        self.platforms_stack.last()
            .map(|s| s.split(',').filter(|x| !x.is_empty()).map(String::from).collect())
            .unwrap_or_default()
    }
    /// Most-recent source override (or `None` if no `git`/`path`
    /// block is active). Push-order wins, so a nested
    /// `git "..." do path "..." do gem end end` correctly
    /// tags the gem with the inner `path`.
    fn active_source_override(&self) -> Option<(String, String)> {
        self.source_stack.last().cloned()
    }

    fn print_summary(&self) {
        println!("Collected Gemfile contents:");
        if let Some(s) = &self.source { println!("  source:        {}", s); }
        if let Some(v) = &self.ruby_version { println!("  ruby version:  {}", v); }
        // Unique-gem count. The per-group sub-headers below add
        // up to *more* than this when a gem appears in multiple
        // groups (e.g. `group :development, :test`), so the
        // label calls out which count this is.
        println!("  gem count (unique): {}", self.gems.len());

        // Bucket by group. `default` is the implicit bucket
        // for any gem declared outside a `group do ... end`.
        let mut by_group: Vec<(String, Vec<&GemDecl>)> = vec![("default".into(), vec![])];
        for g in &self.gems {
            if g.groups.is_empty() {
                by_group[0].1.push(g);
            } else {
                for gn in &g.groups {
                    if let Some(entry) = by_group.iter_mut().find(|(k, _)| k == gn) {
                        entry.1.push(g);
                    } else {
                        by_group.push((gn.clone(), vec![g]));
                    }
                }
            }
        }
        for (group, gems) in &by_group {
            if gems.is_empty() { continue; }
            println!("\n  [{}] {} gem(s):", group, gems.len());
            for g in gems {
                let reqs = if g.requirements.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", g.requirements.join(", "))
                };
                let mut tags: Vec<String> = vec![];
                if !g.require.is_empty() { tags.push(format!("require: {}", g.require)); }
                if !g.platforms_kw.is_empty() { tags.push(format!("platforms: {}", g.platforms_kw)); }
                if let Some((kind, val)) = &g.source_override {
                    tags.push(format!("{}: {}", kind, val));
                }
                if !g.platforms_scope.is_empty() {
                    tags.push(format!("platforms-scope: {}", g.platforms_scope.join(",")));
                }
                let tags_str = if tags.is_empty() { String::new() } else { format!("    [{}]", tags.join(", ")) };
                println!("    - {}{}{}", g.name, reqs, tags_str);
            }
        }
    }
}

/// Pull the inner String out of a Value::Str arg, or "" for any
/// other shape. The prelude only passes Strings into the
/// `__gemfile_*` host fns, so this is the only adapter needed.
fn s(v: &Value) -> String {
    if let Value::Str(rstr) = v { rstr.to_string_lossy() } else { String::new() }
}

fn main() {
    let state = Rc::new(RefCell::new(GemfileState::default()));
    let mut rt = Runtime::new();

    // ---- toplevel DSL hooks (called via the prelude's
    // `source` / `ruby` / `gem` wrappers) ----

    {
        let st = state.clone();
        rt.register_fn("__gemfile_source", move |args| {
            if let [url] = args { st.borrow_mut().source = Some(s(url)); }
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
    // gem_v2 (name: Str, requirements: Array<Str>, opts: Hash<Str, Str>)
    //
    // v2 form: takes the `*requirements` splat as a real Array and
    // the `**opts` kwargs as a real Hash, instead of asking the
    // prelude to flatten everything to Strings. `HostCtx`'s
    // `resolve_array` / `resolve_hash` borrow directly from the
    // heap, no clone.
    //
    // The prelude is a one-liner — `__gemfile_gem_v2(name, requirements,
    // opts)` — with no Ruby-side massage. `HostCtx::resolve_sym` handles
    // the `:require` / `:platforms` Symbol keys; the value side accepts
    // Bool (`require: false`), Sym (`platforms: :mri`), and String
    // (`require: "pry-byebug"`) by direct match. Bundler's full
    // kwargs surface is wider — this demo only consumes the two
    // we care about and silently drops the rest, mirroring how
    // Bundler tolerates unknown options.
    {
        let st = state.clone();
        rt.register_fn_v2("__gemfile_gem_v2", move |ctx: &HostCtx, args| {
            let [name, requirements, opts] = args else {
                return Err(arg_err(format!(
                    "__gemfile_gem_v2: expected 3 args (name, requirements, opts), got {}",
                    args.len()
                )));
            };
            let name = if let Value::Str(rs) = name {
                rs.to_string_lossy()
            } else {
                return Err(arg_err("__gemfile_gem_v2: name must be a String"));
            };
            let reqs_slice = ctx.resolve_array(requirements)
                .ok_or_else(|| arg_err("__gemfile_gem_v2: requirements must be an Array"))?;
            let opts_slice = ctx.resolve_hash(opts)
                .ok_or_else(|| arg_err("__gemfile_gem_v2: opts must be a Hash"))?;

            // Every requirement element must be a String — `gem "rack", ">= 3.0"`
            // could only come through the prelude as a String, so a non-Str
            // element means the prelude regressed.
            let requirements: Vec<String> = reqs_slice.iter()
                .map(|v| if let Value::Str(rs) = v {
                    Ok(rs.to_string_lossy())
                } else {
                    Err(arg_err("__gemfile_gem_v2: requirements element must be a String"))
                })
                .collect::<Result<_, _>>()?;

            // Bundler kwargs Hash: keys are Symbols (`:require`,
            // `:platforms`), values are mixed (Bool, Sym, String).
            // `ctx.resolve_sym(k)` borrows the interned name; the
            // value branch covers each shape we accept and errors
            // explicitly on anything else (e.g. an Integer in
            // `platforms: 3`) so the demo doesn't silently drop data.
            let mut require_kw = String::new();
            let mut platforms_kw = String::new();
            for (k, v) in opts_slice {
                let key = ctx.resolve_sym(k).ok_or_else(|| arg_err(
                    "__gemfile_gem_v2: opts keys must be Symbols"
                ))?;
                let vs = value_to_kwarg_string(ctx, v).ok_or_else(|| arg_err(
                    format!("__gemfile_gem_v2: opts[{key}] must be a Bool, Symbol, or String")
                ))?;
                match key {
                    "require"   => require_kw   = vs,
                    "platforms" => platforms_kw = vs,
                    _ => {} // unknown kwargs ignored — Bundler accepts many we don't model
                }
            }

            let mut state_mut = st.borrow_mut();
            let groups = state_mut.active_groups();
            let platforms_scope = state_mut.active_platforms();
            let source_override = state_mut.active_source_override();
            state_mut.gems.push(GemDecl {
                name,
                requirements,
                groups,
                platforms_scope,
                source_override,
                require: require_kw,
                platforms_kw,
            });
            Ok(Value::Nil)
        });
    }

    // ---- scope-stack push/pop ----

    macro_rules! push_pop {
        ($push_name:expr, $pop_name:expr, $field:ident) => {
            {
                let st = state.clone();
                rt.register_fn($push_name, move |args| {
                    if let [v] = args { st.borrow_mut().$field.push(s(v)); }
                    Ok(Value::Nil)
                });
            }
            {
                let st = state.clone();
                rt.register_fn($pop_name, move |_args| {
                    st.borrow_mut().$field.pop();
                    Ok(Value::Nil)
                });
            }
        };
    }
    push_pop!("__gemfile_push_groups",    "__gemfile_pop_groups",    group_stack);
    push_pop!("__gemfile_push_platforms", "__gemfile_pop_platforms", platforms_stack);

    // `git` and `path` share the unified `source_stack` so the
    // most-recently-entered block wins on nested forms. Push
    // the `(kind, value)` tuple; the matching pop just removes
    // the top entry — the prelude's begin/ensure guarantees
    // pairing.
    macro_rules! source_push_pop {
        ($push_name:expr, $pop_name:expr, $kind:expr) => {
            {
                let st = state.clone();
                rt.register_fn($push_name, move |args| {
                    if let [v] = args {
                        st.borrow_mut().source_stack.push(($kind.into(), s(v)));
                    }
                    Ok(Value::Nil)
                });
            }
            {
                let st = state.clone();
                rt.register_fn($pop_name, move |_args| {
                    st.borrow_mut().source_stack.pop();
                    Ok(Value::Nil)
                });
            }
        };
    }
    source_push_pop!("__gemfile_push_git",  "__gemfile_pop_git",  "git");
    source_push_pop!("__gemfile_push_path", "__gemfile_pop_path", "path");

    // ---- prelude, then the unmodified Gemfile ----

    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("examples/gemfile");
    let prelude_path = base.join("dsl_prelude.rb");
    let gemfile_path = base.join("Gemfile");

    if let Err(trap) = rt.eval_file(&prelude_path) {
        eprintln!("prelude failed:\n{}", rt.format_trap(&trap));
        std::process::exit(1);
    }

    let start = Instant::now();
    if let Err(trap) = rt.eval_file(&gemfile_path) {
        eprintln!("Gemfile failed:\n{}", rt.format_trap(&trap));
        std::process::exit(1);
    }
    let elapsed = start.elapsed();

    state.borrow().print_summary();
    println!();
    println!("rubyrs ran the unmodified Gemfile in {:.2} ms",
        elapsed.as_secs_f64() * 1000.0);
}
