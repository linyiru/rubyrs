//! Bootsnap-style preamble bytecode cache (`preamble-cache` feature).
//!
//! `Runtime::new` spends ~2.7 ms of the CLI's ~5 ms cold start in
//! the pure source→bytecode pipeline (Prism parse → AST translation
//! → `compile_proto`) over the ~176 KB always-on preamble. That
//! pipeline is deterministic for a given binary, so its output —
//! the interner additions, the `Proto` table, and the per-chunk
//! entry indices — is serialized to a host-provided cache directory
//! on first construction and restored on subsequent ones. Preamble
//! EXECUTION (which builds class/method tables and may consult
//! host `Config` capabilities) still happens live on every
//! construction; only compilation is cached.
//!
//! ## Why the cache can never serve stale bytecode
//!
//! The cache key hashes the current executable's identity (length +
//! mtime, via `std::env::current_exe`) plus the crate version plus
//! the PRE-preamble interner contents (which vary with
//! `Config::load_paths` seeding — see `cache_key`). Preamble
//! sources are `include_str!`-baked into the executable, and the
//! bytecode format is whatever this build's `Op`/`Proto` layout
//! is — both are covered by the exe identity, so a blob is only
//! ever decoded by the exact binary that encoded it. Any mismatch
//! (different build, different pre-state, corrupt file, partial
//! write) falls back to the live compile path silently: the cache
//! is a pure fast-path, never a correctness dependency.
//!
//! ## Capability posture (ADR 0017)
//!
//! Library `Runtime`s never touch the filesystem: the cache only
//! engages when the host sets `Config::preamble_cache_dir`. The
//! CLI binary opts in (defaulting to `$RUBYRS_CACHE_DIR` /
//! `$XDG_CACHE_HOME/rubyrs` / `~/.cache/rubyrs`); `RUBYRS_NO_PREAMBLE_CACHE=1`
//! turns it back off. This knob is deliberately separate from
//! `Config::allow_filesystem_io`, which gates SCRIPT-level IO —
//! providing a cache directory is itself the host's consent.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::bytecode::Proto;
use crate::intern::SymId;
use crate::vm::Vm;

/// Sentinel in `steps` marking the point where
/// `install_kernel_builtins` + `install_basic_object_builtins`
/// run between preamble chunks (they intern method names, so
/// their position in the sequence is order-significant).
pub(crate) const STEP_INSTALL_BUILTINS: u32 = u32::MAX;

const MAGIC: &[u8; 4] = b"RBPC";
const FORMAT_VERSION: u32 = 1;

/// Owned (deserialize) shape. `SnapshotRef` below is the borrow
/// twin used at encode time so `store` doesn't clone the proto
/// table.
#[derive(serde::Deserialize)]
struct Snapshot {
    /// `vm.interner.len()` at `load_preamble` entry when the blob
    /// was stored. Restore verifies the live prefix matches
    /// (length AND contents) before appending the rest — SymIds
    /// are positional, so any prefix drift would mis-bind every
    /// symbol the preamble bytecode references.
    pre_interner_len: u32,
    /// `vm.protos.len()` at `load_preamble` entry (expected 0).
    pre_protos_len: u32,
    /// Full interner table (prefix included) in id order.
    interner: Vec<String>,
    /// Full proto table at preamble completion.
    protos: Vec<Proto>,
    /// `vm.cache_counter` at preamble completion (sizes the
    /// inline-cache vector).
    cache_counter: u32,
    /// Replay program: entry proto index per preamble chunk, in
    /// chunk order, with `STEP_INSTALL_BUILTINS` marking the
    /// host-side builtin-install step.
    steps: Vec<u32>,
    /// `vm.sources` pairs (filename, source) for backtrace
    /// resolution — the live path inserts these in `eval_inner`.
    sources: Vec<(String, String)>,
}

#[derive(serde::Serialize)]
struct SnapshotRef<'a> {
    pre_interner_len: u32,
    pre_protos_len: u32,
    interner: Vec<&'a str>,
    protos: &'a [Proto],
    cache_counter: u32,
    steps: &'a [u32],
    sources: Vec<(&'a str, &'a str)>,
}

/// The fields `try_load` hands back for the Runtime to replay.
pub(crate) struct ReplayPlan {
    pub(crate) steps: Vec<u32>,
}

fn fx_hash_bytes(h: &mut crate::intern::FxHasher, bytes: &[u8]) {
    use std::hash::Hasher;
    h.write(bytes);
}

/// Cache key for the current process + pre-preamble state. `None`
/// disables the cache for this construction (e.g. `current_exe`
/// unavailable on the platform).
pub(crate) fn cache_key(vm: &Vm) -> Option<u64> {
    use std::hash::Hasher;
    let exe = std::env::current_exe().ok()?;
    let meta = std::fs::metadata(&exe).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let mut h = crate::intern::FxHasher::default();
    fx_hash_bytes(&mut h, env!("CARGO_PKG_VERSION").as_bytes());
    h.write_u64(meta.len());
    h.write_u64(mtime.as_secs());
    h.write_u32(mtime.subsec_nanos());
    // Pre-preamble interner contents: `Vm::new`'s pre-interned
    // symbols plus whatever `Config::load_paths` seeding interned
    // (`$LOAD_PATH`). Two Runtimes with different pre-state get
    // different keys and therefore different cache files — both
    // valid, neither poisoning the other.
    h.write_usize(vm.interner.len());
    for i in 0..vm.interner.len() {
        fx_hash_bytes(&mut h, vm.interner.resolve(SymId(i as u32)).as_bytes());
    }
    Some(h.finish())
}

fn cache_file(dir: &Path, key: u64) -> PathBuf {
    dir.join(format!("preamble-{key:016x}.bin"))
}

/// Miss-stage telemetry under `RUBYRS_STARTUP_PROF=1` — names which
/// gate rejected the blob so cache problems are diagnosable without
/// a debugger.
fn dbg_miss(stage: &str) {
    if std::env::var_os("RUBYRS_STARTUP_PROF").is_some() {
        eprintln!("startup-prof: preamble-cache miss at: {stage}");
    }
}

/// Try to restore a snapshot into `vm`. On hit, applies the
/// interner / protos / call-cache sizing / sources and returns the
/// replay plan; the caller runs the plan's entry protos (and the
/// builtin-install sentinel) in order. Any mismatch returns `None`
/// and leaves `vm` untouched, so the caller falls back to the live
/// compile path.
pub(crate) fn try_load(vm: &mut Vm, dir: &Path, key: u64) -> Option<ReplayPlan> {
    let Ok(bytes) = std::fs::read(cache_file(dir, key)) else { dbg_miss("read"); return None };
    if bytes.len() < 16 || &bytes[0..4] != MAGIC {
        return None;
    }
    if u32::from_le_bytes(bytes[4..8].try_into().ok()?) != FORMAT_VERSION {
        return None;
    }
    if u64::from_le_bytes(bytes[8..16].try_into().ok()?) != key {
        return None;
    }
    let snap: Snapshot = match postcard::from_bytes(&bytes[16..]) {
        Ok(s) => s,
        Err(e) => { dbg_miss(&format!("decode: {e}")); return None }
    };
    // Verify the pre-preamble state matches what the blob was
    // stored against. The key already hashes all of this; the
    // explicit re-check is belt-and-braces against hash collision
    // and costs ~50 string compares.
    if vm.protos.len() as u32 != snap.pre_protos_len {
        return None;
    }
    if vm.interner.len() as u32 != snap.pre_interner_len {
        return None;
    }
    if snap.interner.len() < snap.pre_interner_len as usize {
        return None;
    }
    for i in 0..vm.interner.len() {
        if &**vm.interner.resolve(SymId(i as u32)) != snap.interner[i].as_str() {
            return None;
        }
    }
    // Apply. From here on the snapshot is committed — every step
    // below is infallible (or panics on ICE, same as the live
    // path's `.expect`).
    for s in &snap.interner[snap.pre_interner_len as usize..] {
        vm.interner.intern(s);
    }
    debug_assert_eq!(vm.interner.len(), snap.interner.len());
    vm.protos = snap.protos;
    vm.cache_counter = snap.cache_counter;
    vm.ensure_call_caches(snap.cache_counter as usize);
    for (f, src) in snap.sources {
        vm.sources.insert(Rc::from(f.as_str()), Rc::from(src.as_str()));
    }
    Some(ReplayPlan { steps: snap.steps })
}

/// Serialize the post-preamble compile state. Best-effort: any IO
/// or encode failure is swallowed (the cache is an optimisation,
/// and the next construction simply compiles live again).
/// `key` MUST be the pre-preamble key computed at `load_preamble`
/// entry — `cache_key` hashes the interner contents, which by
/// store time include every preamble symbol; recomputing here
/// would produce a key `try_load` (which runs pre-preamble) can
/// never reproduce.
pub(crate) fn store(
    vm: &Vm,
    dir: &Path,
    key: u64,
    pre_interner_len: u32,
    pre_protos_len: u32,
    steps: &[u32],
) {
    let snap = SnapshotRef {
        pre_interner_len,
        pre_protos_len,
        interner: (0..vm.interner.len())
            .map(|i| &**vm.interner.resolve(SymId(i as u32)))
            .collect(),
        protos: &vm.protos,
        cache_counter: vm.cache_counter,
        steps,
        sources: vm
            .sources
            .iter()
            .map(|(k, v)| (&**k, &**v))
            .collect(),
    };
    let Ok(body) = postcard::to_allocvec(&snap) else { return };
    let mut bytes = Vec::with_capacity(16 + body.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&key.to_le_bytes());
    bytes.extend_from_slice(&body);
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // Atomic publish: write to a pid-suffixed temp file then
    // rename. Concurrent constructors either see the old blob, the
    // new blob, or no blob — never a torn one.
    let tmp = dir.join(format!(
        "preamble-{key:016x}.tmp.{}",
        std::process::id(),
    ));
    if std::fs::write(&tmp, &bytes).is_err() {
        let _ = std::fs::remove_file(&tmp);
        return;
    }
    let _ = std::fs::rename(&tmp, cache_file(dir, key));
}

/// The CLI's default cache directory: `$RUBYRS_CACHE_DIR`, else
/// `$XDG_CACHE_HOME/rubyrs`, else `$HOME/.cache/rubyrs`, else
/// `None` (cache disabled). Exposed for the CLI binary; library
/// embedders pass an explicit directory via
/// `Config::preamble_cache_dir` instead.
pub fn default_cache_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("RUBYRS_CACHE_DIR") {
        return Some(PathBuf::from(d));
    }
    if let Some(d) = std::env::var_os("XDG_CACHE_HOME") {
        return Some(PathBuf::from(d).join("rubyrs"));
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache").join("rubyrs"))
}

#[cfg(test)]
mod tests {

    /// Round-trip the snapshot encoding through a real Vm pair:
    /// store from one freshly-preambled Runtime, load into a
    /// second, and check the second produces identical eval
    /// results. Uses a tempdir so parallel test runs don't share
    /// state.
    #[test]
    fn snapshot_roundtrip_via_runtime() {
        let dir = std::env::temp_dir().join(format!(
            "rubyrs-pc-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);

        let mk = || {
            crate::Runtime::with_config(crate::Config {
                preamble_cache_dir: Some(dir.clone()),
                ..Default::default()
            })
        };
        // First construction: cache miss → live compile → store.
        let mut a = mk();
        assert!(!a.preamble_cache_hit());
        // Second: must hit and behave identically.
        let mut b = mk();
        assert!(b.preamble_cache_hit(), "second construction should hit the cache");

        let probe = r#"
            class PcProbe
              def initialize(n); @n = n; end
              def go(k: 2); [@n * k, "s-#{@n}".upcase, (1..3).map { |i| i + @n }]; end
            end
            begin
              raise ArgumentError, "boom" if PcProbe.new(3).go.first != 6
              PcProbe.new(4).go(k: 10).inspect
            rescue ArgumentError => e
              "rescued: #{e.message}"
            end
        "#;
        let va = a.eval(probe, "probe.rb").expect("live runtime eval");
        let vb = b.eval(probe, "probe.rb").expect("cached runtime eval");
        assert_eq!(format!("{va:?}"), format!("{vb:?}"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A corrupt cache file must fall back to live compile, not
    /// panic or mis-restore.
    #[test]
    fn corrupt_blob_falls_back_to_live() {
        let dir = std::env::temp_dir().join(format!(
            "rubyrs-pc-corrupt-{}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mk = || {
            crate::Runtime::with_config(crate::Config {
                preamble_cache_dir: Some(dir.clone()),
                ..Default::default()
            })
        };
        let _ = mk(); // populate
        // Truncate / scribble every cache file in the dir.
        for ent in std::fs::read_dir(&dir).unwrap().flatten() {
            std::fs::write(ent.path(), b"RBPCgarbage").unwrap();
        }
        let mut rt = mk();
        assert!(!rt.preamble_cache_hit());
        let v = rt.eval("[1, 2, 3].sum", "p.rb").expect("eval after fallback");
        assert_eq!(format!("{v:?}"), "Int(6)");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
