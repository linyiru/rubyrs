//! `_sqlite` battery — rusqlite wrapper per ADR 0027.
//!
//! Phase 3 of menu item 4 (per ADR 0026 v2). Ships:
//!
//!   - Per-thread `SQLITE_CONNS: HashMap<i64, ConnState>` keyed
//!     by opaque integer handles. The rubyrs VM is
//!     single-threaded so per-thread = per-Vm in practice.
//!   - 9 host fns registered via `register_host_fns`:
//!       __rubyrs_sqlite_open(path, opts) → handle
//!       __rubyrs_sqlite_close(handle) → nil
//!       __rubyrs_sqlite_execute(handle, sql, params) → rows_changed
//!       __rubyrs_sqlite_execute_cached(handle, sql, params) → rows_changed
//!       __rubyrs_sqlite_query(handle, sql, params) → Array<Array<Value>>
//!       __rubyrs_sqlite_query_cached(handle, sql, params) → ditto
//!       __rubyrs_sqlite_busy_timeout(handle, ms) → nil
//!       __rubyrs_sqlite_cache_hits(handle) → Integer
//!       __rubyrs_sqlite_cache_misses(handle) → Integer
//!   - The 25-class `SQLite3::Exception` hierarchy + the
//!     Ruby-side `SQLite3::Database` class loaded from
//!     `preamble/sqlite_database.rb` at host-fn-registration
//!     time.
//!
//! Connection-state field order (ADR 0027 §4) is load-bearing:
//! `stmts` (containing `Statement<'static>` values whose true
//! borrow is `conn`) MUST appear before `conn` so Rust's
//! declaration-order Drop drops them in the right sequence
//! (statements finalized BEFORE the Connection they borrowed
//! from is closed). Reversing this is UB on shutdown.

#![cfg(feature = "_sqlite")]

use crate::error::{RubyError, Trap};
use crate::heap::{HashObj, HeapObj};
use crate::value::Value;
use crate::vm::current_vm_ptr;
use lru::LruCache;
use rusqlite::{Connection, ErrorCode, OpenFlags, Statement};
use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_BUSY_TIMEOUT_MS: u32 = 5000;
const DEFAULT_CACHE_CAP: usize = 100;

/// Per-connection state. **Field order is LOAD-BEARING**: Rust
/// drops struct fields in declaration order, and the cached
/// `Statement<'static>` values borrow (via unsafe transmute)
/// from `conn`. `stmts` must drop first so each Statement gets
/// `sqlite3_finalize`'d while its Connection is still alive.
/// Reversing the field order is UB on shutdown. ADR 0027 §4.
pub(crate) struct ConnState {
    pub(crate) stmts: LruCache<String, Statement<'static>>,
    pub(crate) conn: Connection,
    pub(crate) cache_hits: u64,
    pub(crate) cache_misses: u64,
    /// Re-entrancy guard for `execute_cached` / `query_cached`.
    /// Set true on entry, false on exit; recursive entry traps
    /// `SQLite3::MisuseException` so LRU eviction can't fire
    /// mid-borrow. ADR 0027 §4 "Re-entrancy hazard."
    pub(crate) prepare_active: bool,
}

/// Standalone prepared statement returned by
/// `Database#prepare(sql)`. Holds the `Statement<'static>`
/// transmuted from `Statement<'conn>` PLUS a weak handle back
/// to its owning Connection so we can validate "the Connection
/// is still alive" on every call without making the statement
/// own the Connection. ADR 0027 §"Surface freeze policy" v2
/// extension — the user-visible `SQLite3::Statement` class
/// goes here. Closes the `select_one_cached` 12 % bench gap
/// vs CRuby by skipping the per-call SQL-string → LRU lookup
/// (each `stmt.execute(args)` hops straight to bind + step).
pub(crate) struct OwnedStmt {
    pub(crate) stmt: Statement<'static>,
    /// Handle of the Connection this Statement borrows from.
    /// Validated at every call: if the Connection's been closed
    /// (handle removed from SQLITE_CONNS), the Statement's
    /// `'static` lifetime is now dangling and any call traps
    /// `SQLite3::Exception` ("statement orphaned: connection
    /// was closed").
    pub(crate) owner_handle: i64,
}

thread_local! {
    /// Connection map keyed by opaque handle. Per-thread; the
    /// rubyrs VM is single-threaded so this is effectively
    /// per-Vm. ADR 0027 §"Capability host-fns consumed".
    static SQLITE_CONNS: RefCell<HashMap<i64, ConnState>> = RefCell::new(HashMap::new());
    /// Prepared-statement map keyed by opaque handle (distinct
    /// from connection handles — the two spaces share
    /// `NEXT_HANDLE` so a handle is never reused across types).
    /// The `OwnedStmt::stmt` field is `Statement<'static>`,
    /// transmuted from a real `Statement<'conn>`. Drop ordering
    /// here is more subtle than the in-Connection LRU's case:
    /// these statements outlive their `prepare()` call frame,
    /// so the runtime invariant is "STMT_HANDLES entry must be
    /// removed BEFORE the SQLITE_CONNS entry for the same
    /// `owner_handle`." `__rubyrs_sqlite_close` enforces this
    /// by sweeping orphaned Statements before removing the
    /// Connection. Explicit `stmt.close` removes its entry
    /// individually.
    static STMT_HANDLES: RefCell<HashMap<i64, OwnedStmt>> = RefCell::new(HashMap::new());
    static NEXT_HANDLE: std::cell::Cell<i64> = const { std::cell::Cell::new(1) };
}

/// Register the `_sqlite` host fns + load the
/// `SQLite3::Database` preamble. Call once per Runtime that
/// wants the battery. Mirrors `register_http_server_host_fns` /
/// `register_json_native_host_fns` shape.
pub fn register_host_fns(rt: &mut crate::Runtime) {
    // Preamble: SQLite3 module + 25 exception subclasses +
    // SQLite3::Database class. Authored as Ruby source so the
    // 25 empty `class Foo < SQLite3::Exception; end` lines stay
    // readable + the Database wrapper class doesn't need a
    // round trip through host-fn dispatch for every method call.
    const PREAMBLE: &str = include_str!("preamble/sqlite_database.rb");
    if let Err(trap) = rt.eval(PREAMBLE, "<rubyrs:sqlite_database>") {
        panic!("ICE: _sqlite failed to load preamble: {trap:?}");
    }

    rt.register_fn("__rubyrs_sqlite_open", |args| {
        let (path_str, opts) = match args {
            [Value::Str(p)] => (p.to_string_lossy(), None),
            [Value::Str(p), Value::Hash(h)] => (p.to_string_lossy(), Some(*h)),
            _ => return Err(arg_err("__rubyrs_sqlite_open(path: String[, opts: Hash])")),
        };
        // Optional `:busy_timeout_ms` override; default 5000.
        let busy_ms = opts
            .and_then(|hid| hash_get_int(hid, "busy_timeout_ms"))
            .unwrap_or(DEFAULT_BUSY_TIMEOUT_MS as i64);
        // Optional `:cache_size`; default 100. Tunable per ADR 0027 §4.
        let cache_cap = opts
            .and_then(|hid| hash_get_int(hid, "cache_size"))
            .unwrap_or(DEFAULT_CACHE_CAP as i64);

        check_path_allowed(&path_str)?;

        let conn = open_connection(&path_str)
            .map_err(|e| map_sqlite_err(e, "opening database"))?;
        if busy_ms > 0 {
            conn.busy_timeout(Duration::from_millis(busy_ms as u64))
                .map_err(|e| map_sqlite_err(e, "setting busy_timeout"))?;
        }
        let cap = NonZeroUsize::new(cache_cap.max(1) as usize).unwrap();

        let handle = NEXT_HANDLE.with(|c| {
            let h = c.get();
            c.set(h + 1);
            h
        });
        SQLITE_CONNS.with(|m| {
            m.borrow_mut().insert(handle, ConnState {
                stmts: LruCache::new(cap),
                conn,
                cache_hits: 0,
                cache_misses: 0,
                prepare_active: false,
            });
        });
        Ok(Value::Int(handle))
    });

    rt.register_fn("__rubyrs_sqlite_close", |args| {
        let handle = handle_arg(args, "__rubyrs_sqlite_close(handle)")?;
        // SWEEP first: any STMT_HANDLES entry whose
        // `owner_handle == handle` must drop BEFORE the
        // SQLITE_CONNS entry, because the Statement<'static>
        // inside has its true 'conn borrow pointing into the
        // Connection we're about to drop. Sweep removes the
        // statements (each drop calls sqlite3_finalize on the
        // live conn), THEN we drop the Connection.
        STMT_HANDLES.with(|m| {
            let mut map = m.borrow_mut();
            map.retain(|_sh, owned| owned.owner_handle != handle);
        });
        SQLITE_CONNS.with(|m| {
            // `remove` drops the ConnState, which drops `stmts`
            // (finalising each cached LRU Statement) THEN
            // `conn` — load-bearing field order from §4.
            m.borrow_mut().remove(&handle);
        });
        Ok(Value::Nil)
    });

    rt.register_fn("__rubyrs_sqlite_execute", |args| {
        let (handle, sql, params) = parse_exec_args(args, "execute")?;
        exec_impl(handle, &sql, params, false).map(Value::Int)
    });

    rt.register_fn("__rubyrs_sqlite_execute_cached", |args| {
        let (handle, sql, params) = parse_exec_args(args, "execute_cached")?;
        exec_impl(handle, &sql, params, true).map(Value::Int)
    });

    rt.register_fn("__rubyrs_sqlite_query", |args| {
        let (handle, sql, params) = parse_exec_args(args, "query")?;
        query_impl(handle, &sql, params, false)
    });

    rt.register_fn("__rubyrs_sqlite_query_cached", |args| {
        let (handle, sql, params) = parse_exec_args(args, "query_cached")?;
        query_impl(handle, &sql, params, true)
    });

    rt.register_fn("__rubyrs_sqlite_busy_timeout", |args| {
        let (handle, ms) = match args {
            [Value::Int(h), Value::Int(ms)] => (*h, *ms),
            _ => return Err(arg_err("__rubyrs_sqlite_busy_timeout(handle, ms)")),
        };
        SQLITE_CONNS.with(|m| -> Result<Value, Trap> {
            let mut map = m.borrow_mut();
            let st = map.get_mut(&handle).ok_or_else(closed_db)?;
            st.conn
                .busy_timeout(Duration::from_millis(ms.max(0) as u64))
                .map_err(|e| map_sqlite_err(e, "setting busy_timeout"))?;
            Ok(Value::Nil)
        })
    });

    rt.register_fn("__rubyrs_sqlite_cache_hits", |args| {
        let handle = handle_arg(args, "__rubyrs_sqlite_cache_hits(handle)")?;
        SQLITE_CONNS.with(|m| -> Result<Value, Trap> {
            let map = m.borrow();
            let st = map.get(&handle).ok_or_else(closed_db)?;
            Ok(Value::Int(st.cache_hits as i64))
        })
    });

    rt.register_fn("__rubyrs_sqlite_cache_misses", |args| {
        let handle = handle_arg(args, "__rubyrs_sqlite_cache_misses(handle)")?;
        SQLITE_CONNS.with(|m| -> Result<Value, Trap> {
            let map = m.borrow();
            let st = map.get(&handle).ok_or_else(closed_db)?;
            Ok(Value::Int(st.cache_misses as i64))
        })
    });

    // ---- Prepared-statement opcodes (Phase 3.1) ----
    // Closes the select_one_cached bench gap by exposing the
    // CRuby-shape prepare-once pattern. ADR 0027 §"Surface
    // freeze policy" v2 extension. The four ops mirror the
    // Database ones but key on a per-Statement handle held by
    // the SQLite3::Statement Ruby class.

    rt.register_fn("__rubyrs_sqlite_prepare", |args| {
        let (handle, sql) = match args {
            [Value::Int(h), Value::Str(s)] => (*h, s.to_string_lossy()),
            _ => return Err(arg_err("__rubyrs_sqlite_prepare(handle, sql)")),
        };
        let stmt_handle = SQLITE_CONNS.with(|m| -> Result<i64, Trap> {
            let map = m.borrow();
            let st = map.get(&handle).ok_or_else(closed_db)?;
            // SAFETY: Statement borrows from Connection. The
            // transmute pairs with the runtime invariant
            // "STMT_HANDLES entry for owner_handle=H must be
            // removed before SQLITE_CONNS removes H." Enforced
            // at the close site via sweep_orphaned_stmts.
            let stmt = unsafe {
                let real: Statement<'_> = st.conn
                    .prepare(&sql)
                    .map_err(|e| map_sqlite_err(e, "prepare"))?;
                std::mem::transmute::<Statement<'_>, Statement<'static>>(real)
            };
            let sh = NEXT_HANDLE.with(|c| {
                let h = c.get();
                c.set(h + 1);
                h
            });
            STMT_HANDLES.with(|m| {
                m.borrow_mut().insert(sh, OwnedStmt { stmt, owner_handle: handle });
            });
            Ok(sh)
        })?;
        Ok(Value::Int(stmt_handle))
    });

    rt.register_fn("__rubyrs_sqlite_stmt_execute", |args| {
        let (stmt_handle, params) = parse_stmt_args(args, "stmt_execute")?;
        with_stmt(stmt_handle, |st_stmt, vm| {
            st_stmt.stmt.clear_bindings();
            bind_params(&mut st_stmt.stmt, &params, vm)?;
            st_stmt.stmt
                .raw_execute()
                .map(|n| Value::Int(n as i64))
                .map_err(|e| map_sqlite_err(e, "stmt execute"))
        })
    });

    rt.register_fn("__rubyrs_sqlite_stmt_query", |args| {
        let (stmt_handle, params) = parse_stmt_args(args, "stmt_query")?;
        let max_bytes = with_vm(|vm| vm.sqlite_max_result_bytes);
        with_stmt(stmt_handle, |st_stmt, vm| {
            st_stmt.stmt.clear_bindings();
            bind_params(&mut st_stmt.stmt, &params, vm)?;
            collect_rows(&mut st_stmt.stmt, vm, max_bytes)
        })
    });

    rt.register_fn("__rubyrs_sqlite_stmt_close", |args| {
        let stmt_handle = handle_arg(args, "__rubyrs_sqlite_stmt_close(stmt_handle)")?;
        STMT_HANDLES.with(|m| {
            m.borrow_mut().remove(&stmt_handle);
        });
        Ok(Value::Nil)
    });
}

// ---- helpers ----

fn arg_err(msg: &str) -> Trap {
    Trap {
        err: RubyError::ArgumentError { msg: msg.to_string() },
        backtrace: vec![],
    }
}

fn closed_db() -> Trap {
    Trap {
        err: RubyError::HostException {
            class_name: "SQLite3::Exception".to_string(),
            message: "closed database".to_string(),
        },
        backtrace: vec![],
    }
}

fn handle_arg(args: &[Value], shape: &str) -> Result<i64, Trap> {
    match args {
        [Value::Int(h)] => Ok(*h),
        _ => Err(arg_err(shape)),
    }
}

/// Parameter-shape parser for the statement-handle ops
/// (`stmt_execute` / `stmt_query`). Same shape as
/// `parse_exec_args` but the SQL string is implicit (owned by
/// the Statement) so we only take handle + params.
fn parse_stmt_args(args: &[Value], op: &str) -> Result<(i64, Vec<Value>), Trap> {
    match args {
        [Value::Int(sh)] => Ok((*sh, vec![])),
        [Value::Int(sh), Value::Array(p)] => {
            let vm = unsafe { &mut *current_vm_ptr() };
            let params: Vec<Value> = vm.heap.array(*p).clone();
            Ok((*sh, params))
        }
        _ => Err(arg_err(&format!("__rubyrs_sqlite_{op}(stmt_handle[, params])"))),
    }
}

/// Borrow a Statement by handle, run a closure that mutates it
/// + the Vm, return the result. Centralises the
/// STMT_HANDLES → Vm-borrow dance so the per-host-fn closures
/// don't repeat it.
fn with_stmt<F>(stmt_handle: i64, f: F) -> Result<Value, Trap>
where
    F: FnOnce(&mut OwnedStmt, &mut crate::vm::Vm) -> Result<Value, Trap>,
{
    let ptr = current_vm_ptr();
    if ptr.is_null() {
        return Err(arg_err("sqlite host fn: VM ptr null"));
    }
    let vm = unsafe { &mut *ptr };
    STMT_HANDLES.with(|m| -> Result<Value, Trap> {
        let mut map = m.borrow_mut();
        let owned = map.get_mut(&stmt_handle).ok_or_else(|| Trap {
            err: RubyError::HostException {
                class_name: "SQLite3::Exception".to_string(),
                message: "closed statement (or invalid handle)".to_string(),
            },
            backtrace: vec![],
        })?;
        // Validate the owning Connection is still alive — if
        // the Database#close swept us, the handle is gone from
        // the local map already (see the close-site retain); a
        // dangling Statement would have been dropped there. But
        // defensive double-check.
        let alive = SQLITE_CONNS.with(|cm| cm.borrow().contains_key(&owned.owner_handle));
        if !alive {
            return Err(Trap {
                err: RubyError::HostException {
                    class_name: "SQLite3::Exception".to_string(),
                    message: "statement orphaned: owning database was closed".to_string(),
                },
                backtrace: vec![],
            });
        }
        f(owned, vm)
    })
}

/// Short-hand for read-only Vm access from a host fn closure.
fn with_vm<F, R>(f: F) -> R
where
    F: FnOnce(&crate::vm::Vm) -> R,
{
    let ptr = current_vm_ptr();
    let vm = unsafe { &*ptr };
    f(vm)
}

fn parse_exec_args(args: &[Value], op: &str) -> Result<(i64, String, Vec<Value>), Trap> {
    match args {
        [Value::Int(h), Value::Str(sql)] => Ok((*h, sql.to_string_lossy(), vec![])),
        [Value::Int(h), Value::Str(sql), Value::Array(p)] => {
            let ptr = current_vm_ptr();
            if ptr.is_null() {
                return Err(arg_err("sqlite host fn: VM ptr null"));
            }
            // SAFETY: dispatch site sets CURRENT_VM_PTR for the host-fn invocation window.
            let vm = unsafe { &mut *ptr };
            let params: Vec<Value> = vm.heap.array(*p).clone();
            Ok((*h, sql.to_string_lossy(), params))
        }
        _ => Err(arg_err(&format!("__rubyrs_sqlite_{op}(handle, sql[, params])"))),
    }
}

/// Lookup `key` in Hash `id`, return as i64 if it's an Int. The
/// per-call `opts` hash uses Symbol keys; we match those.
fn hash_get_int(hash_id: crate::value::ObjId, key: &str) -> Option<i64> {
    let ptr = current_vm_ptr();
    if ptr.is_null() {
        return None;
    }
    let vm = unsafe { &*ptr };
    let pairs = vm.heap.hash(hash_id);
    for (k, v) in pairs {
        let matches_key = match k {
            Value::Sym(s) => vm.interner.resolve(*s).as_ref() == key,
            Value::Str(s) => s.to_string_lossy() == key,
            _ => false,
        };
        if matches_key {
            if let Value::Int(n) = v {
                return Some(*n);
            }
        }
    }
    None
}

/// Open a `Connection` with URI parsing enabled for any path
/// matching `^file:` (so `file::memory:?cache=shared` and
/// `file:foo?mode=memory` Just Work) and literal-path mode for
/// everything else. ADR 0027 §7.
fn open_connection(path: &str) -> rusqlite::Result<Connection> {
    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    if path.starts_with("file:") {
        flags |= OpenFlags::SQLITE_OPEN_URI;
    }
    Connection::open_with_flags(path, flags)
}

/// Path-sandbox check per ADR 0027 §7. Returns Ok for allowed
/// paths, Err with SQLite3::CantOpenException for blocked ones.
fn check_path_allowed(path: &str) -> Result<(), Trap> {
    // `:memory:` and URI in-memory forms are unconditionally
    // allowed (no FS reach).
    if path == ":memory:" || is_uri_memory_form(path) {
        return Ok(());
    }
    let ptr = current_vm_ptr();
    if ptr.is_null() {
        return Ok(()); // can't check; assume embedder context lets it through
    }
    let vm = unsafe { &*ptr };

    if !vm.allow_filesystem_io {
        return Err(Trap {
            err: RubyError::HostException {
                class_name: "SQLite3::CantOpenException".to_string(),
                message: format!(
                    "sandbox blocked: SQLite open of {:?} (Config::allow_filesystem_io is false)",
                    path
                ),
            },
            backtrace: vec![],
        });
    }
    let allowed = vm.sqlite_allow_paths.as_ref();
    let prefixes: Option<&Vec<PathBuf>> = allowed.map(|v| v.as_ref());
    if let Some(prefixes) = prefixes {
        let resolved = crate::lexically_resolve_path(std::path::Path::new(path));
        if !prefixes.iter().any(|p| resolved.starts_with(p)) {
            return Err(Trap {
                err: RubyError::HostException {
                    class_name: "SQLite3::CantOpenException".to_string(),
                    message: format!(
                        "sandbox blocked: SQLite open of {:?} outside Config::sqlite_allow_paths",
                        resolved
                    ),
                },
                backtrace: vec![],
            });
        }
    }
    Ok(())
}

fn is_uri_memory_form(path: &str) -> bool {
    // Common SQLite in-memory URI shapes. Cheap string match —
    // good enough for the sandbox check; the actual URI parsing
    // happens inside libsqlite3.
    path == "file::memory:"
        || path.starts_with("file::memory:?")
        || path.contains("mode=memory")
}

// ---- exec / query implementations ----

fn exec_impl(handle: i64, sql: &str, params: Vec<Value>, use_cache: bool) -> Result<i64, Trap> {
    let ptr = current_vm_ptr();
    if ptr.is_null() {
        return Err(arg_err("sqlite host fn: VM ptr null"));
    }
    let vm = unsafe { &mut *ptr };
    SQLITE_CONNS.with(|m| -> Result<i64, Trap> {
        let mut map = m.borrow_mut();
        let st = map.get_mut(&handle).ok_or_else(closed_db)?;
        if use_cache {
            if st.prepare_active {
                return Err(Trap {
                    err: RubyError::HostException {
                        class_name: "SQLite3::MisuseException".to_string(),
                        message: "recursive execute_cached during in-flight prepare".to_string(),
                    },
                    backtrace: vec![],
                });
            }
            st.prepare_active = true;
            let result = exec_cached_inner(st, sql, &params, vm);
            st.prepare_active = false;
            result
        } else {
            // Uncached path: fresh prepare per call.
            let mut stmt = st.conn.prepare(sql).map_err(|e| map_sqlite_err(e, "prepare"))?;
            bind_params(&mut stmt, &params, vm)?;
            let n = stmt.raw_execute().map_err(|e| map_sqlite_err(e, "execute"))?;
            Ok(n as i64)
        }
    })
}

fn exec_cached_inner(
    st: &mut ConnState,
    sql: &str,
    params: &[Value],
    vm: &mut crate::vm::Vm,
) -> Result<i64, Trap> {
    let cap = st.stmts.cap();
    let conn_ptr: *const Connection = &st.conn;
    let cached = st.stmts.get_mut(sql).is_some();
    if cached {
        st.cache_hits += 1;
    } else {
        st.cache_misses += 1;
        // SAFETY: the Statement we cache borrows from `st.conn`.
        // We transmute its lifetime to `'static` paired with the
        // load-bearing struct-field-drop-order invariant (§4).
        // No external code observes the Statement outliving its
        // Connection.
        let stmt = unsafe {
            let real: Statement<'_> = (*conn_ptr)
                .prepare(sql)
                .map_err(|e| map_sqlite_err(e, "prepare"))?;
            std::mem::transmute::<Statement<'_>, Statement<'static>>(real)
        };
        st.stmts.put(sql.to_string(), stmt);
        let _ = cap;
    }
    let stmt = st.stmts.get_mut(sql).expect("just inserted");
    stmt.clear_bindings();
    bind_params(stmt, params, vm)?;
    let n = stmt.raw_execute().map_err(|e| map_sqlite_err(e, "execute"))?;
    Ok(n as i64)
}

fn query_impl(handle: i64, sql: &str, params: Vec<Value>, use_cache: bool) -> Result<Value, Trap> {
    let ptr = current_vm_ptr();
    if ptr.is_null() {
        return Err(arg_err("sqlite host fn: VM ptr null"));
    }
    let vm = unsafe { &mut *ptr };
    let max_bytes = vm.sqlite_max_result_bytes;

    SQLITE_CONNS.with(|m| -> Result<Value, Trap> {
        let mut map = m.borrow_mut();
        let st = map.get_mut(&handle).ok_or_else(closed_db)?;
        if use_cache {
            if st.prepare_active {
                return Err(Trap {
                    err: RubyError::HostException {
                        class_name: "SQLite3::MisuseException".to_string(),
                        message: "recursive query_cached during in-flight prepare".to_string(),
                    },
                    backtrace: vec![],
                });
            }
            st.prepare_active = true;
            let result = query_cached_inner(st, sql, &params, vm, max_bytes);
            st.prepare_active = false;
            result
        } else {
            let mut stmt = st.conn.prepare(sql).map_err(|e| map_sqlite_err(e, "prepare"))?;
            bind_params(&mut stmt, &params, vm)?;
            collect_rows(&mut stmt, vm, max_bytes)
        }
    })
}

fn query_cached_inner(
    st: &mut ConnState,
    sql: &str,
    params: &[Value],
    vm: &mut crate::vm::Vm,
    max_bytes: Option<usize>,
) -> Result<Value, Trap> {
    let conn_ptr: *const Connection = &st.conn;
    let cached = st.stmts.get_mut(sql).is_some();
    if cached {
        st.cache_hits += 1;
    } else {
        st.cache_misses += 1;
        let stmt = unsafe {
            let real: Statement<'_> = (*conn_ptr)
                .prepare(sql)
                .map_err(|e| map_sqlite_err(e, "prepare"))?;
            std::mem::transmute::<Statement<'_>, Statement<'static>>(real)
        };
        st.stmts.put(sql.to_string(), stmt);
    }
    let stmt = st.stmts.get_mut(sql).expect("just inserted");
    stmt.clear_bindings();
    bind_params(stmt, params, vm)?;
    collect_rows(stmt, vm, max_bytes)
}

/// Bind Ruby `Value`s as positional parameters (1-indexed in
/// SQLite). Type marshalling per ADR 0027 §5.
fn bind_params(stmt: &mut Statement<'_>, params: &[Value], vm: &crate::vm::Vm) -> Result<(), Trap> {
    for (i, v) in params.iter().enumerate() {
        let idx = (i + 1) as usize;
        match v {
            Value::Nil => stmt
                .raw_bind_parameter(idx, rusqlite::types::Null)
                .map_err(|e| map_sqlite_err(e, "bind nil"))?,
            Value::Bool(b) => stmt
                .raw_bind_parameter(idx, if *b { 1i64 } else { 0i64 })
                .map_err(|e| map_sqlite_err(e, "bind bool"))?,
            Value::Int(n) => stmt
                .raw_bind_parameter(idx, *n)
                .map_err(|e| map_sqlite_err(e, "bind int"))?,
            Value::Float(f) => stmt
                .raw_bind_parameter(idx, *f)
                .map_err(|e| map_sqlite_err(e, "bind float"))?,
            Value::Str(s) => {
                let bytes = s.content.borrow();
                let txt = String::from_utf8_lossy(&bytes).into_owned();
                stmt.raw_bind_parameter(idx, txt)
                    .map_err(|e| map_sqlite_err(e, "bind str"))?;
            }
            Value::Sym(sid) => {
                let name = vm.interner.resolve(*sid).to_string();
                stmt.raw_bind_parameter(idx, name)
                    .map_err(|e| map_sqlite_err(e, "bind sym"))?;
            }
            other => {
                return Err(Trap {
                    err: RubyError::HostException {
                        class_name: "SQLite3::MismatchException".to_string(),
                        message: format!(
                            "cannot bind {} as SQLite parameter",
                            other.type_name()
                        ),
                    },
                    backtrace: vec![],
                });
            }
        }
    }
    Ok(())
}

/// Iterate `stmt`'s rows, marshalling each column to a Ruby
/// `Value`. Returns `Value::Array(Value::Array(...))` — an array
/// of rows, each row an array of column values (positional, in
/// declaration order). The Ruby-side wrapper zips column names
/// in.
fn collect_rows(
    stmt: &mut Statement<'_>,
    vm: &mut crate::vm::Vm,
    max_bytes: Option<usize>,
) -> Result<Value, Trap> {
    let col_count = stmt.column_count();
    let mut rows = stmt.raw_query();
    let mut out: Vec<Value> = Vec::new();
    let mut running_bytes: usize = 0;
    while let Some(row) = rows.next().map_err(|e| map_sqlite_err(e, "fetch row"))? {
        let mut cells: Vec<Value> = Vec::with_capacity(col_count);
        for i in 0..col_count {
            let raw = row.get_ref(i).map_err(|e| map_sqlite_err(e, "get column"))?;
            let (val, bytes) = sqlite_to_value(&raw, vm)?;
            running_bytes = running_bytes.saturating_add(bytes);
            if let Some(cap) = max_bytes
                && running_bytes > cap
            {
                return Err(Trap {
                    err: RubyError::HostException {
                        class_name: "SQLite3::TooBigException".to_string(),
                        message: format!(
                            "query result exceeds Config::sqlite_max_result_bytes ({} > {})",
                            running_bytes, cap
                        ),
                    },
                    backtrace: vec![],
                });
            }
            cells.push(val);
        }
        // Allocate row array on heap. The cells Vec is fresh and rooted via Rust ownership;
        // no Value::Str / Array allocated above survives without being in cells, so no GC
        // root issue.
        let row_id = vm.heap.alloc(HeapObj::Array(cells));
        out.push(Value::Array(row_id));
    }
    let arr_id = vm.heap.alloc(HeapObj::Array(out));
    let _ = HashObj::with_pairs; // suppress dead-code warning under one cfg combo
    Ok(Value::Array(arr_id))
}

fn sqlite_to_value(
    raw: &rusqlite::types::ValueRef<'_>,
    _vm: &mut crate::vm::Vm,
) -> Result<(Value, usize), Trap> {
    use rusqlite::types::ValueRef;
    Ok(match raw {
        ValueRef::Null => (Value::Nil, 1),
        ValueRef::Integer(n) => (Value::Int(*n), 8),
        ValueRef::Real(f) => (Value::Float(*f), 8),
        ValueRef::Text(bytes) => {
            let s = String::from_utf8_lossy(bytes).into_owned();
            let len = s.len();
            (Value::new_str(s), len + 24)
        }
        ValueRef::Blob(bytes) => {
            let v = Value::new_str_bytes(bytes.to_vec());
            (v, bytes.len() + 24)
        }
    })
}

// ---- error mapping ----

/// Map a rusqlite::Error onto one of the 25 SQLite3::*Exception
/// classes. ADR 0027 §6 — error message is the native SQLite
/// string forwarded verbatim from libsqlite3.
fn map_sqlite_err(e: rusqlite::Error, context: &str) -> Trap {
    let (class_name, msg) = classify(&e, context);
    Trap {
        err: RubyError::HostException { class_name, message: msg },
        backtrace: vec![],
    }
}

fn classify(e: &rusqlite::Error, context: &str) -> (String, String) {
    use rusqlite::Error::*;
    let msg = format!("{context}: {e}");
    let cls = match e {
        SqliteFailure(err, _) => match err.code {
            ErrorCode::ConstraintViolation => "SQLite3::ConstraintException",
            ErrorCode::CannotOpen => "SQLite3::CantOpenException",
            ErrorCode::DatabaseBusy => "SQLite3::BusyException",
            ErrorCode::DatabaseLocked => "SQLite3::LockedException",
            ErrorCode::ReadOnly => "SQLite3::ReadOnlyException",
            ErrorCode::DatabaseCorrupt => "SQLite3::CorruptException",
            ErrorCode::DiskFull => "SQLite3::FullException",
            ErrorCode::SystemIoFailure => "SQLite3::IOException",
            ErrorCode::PermissionDenied => "SQLite3::PermissionException",
            ErrorCode::OutOfMemory => "SQLite3::MemoryException",
            ErrorCode::TypeMismatch => "SQLite3::MismatchException",
            ErrorCode::ApiMisuse => "SQLite3::MisuseException",
            ErrorCode::NotFound => "SQLite3::NotFoundException",
            ErrorCode::Unknown => "SQLite3::SQLException",
            ErrorCode::OperationAborted => "SQLite3::AbortException",
            ErrorCode::OperationInterrupted => "SQLite3::InterruptException",
            ErrorCode::SchemaChanged => "SQLite3::SchemaChangedException",
            ErrorCode::TooBig => "SQLite3::TooBigException",
            ErrorCode::ParameterOutOfRange => "SQLite3::RangeException",
            ErrorCode::AuthorizationForStatementDenied => "SQLite3::AuthorizationException",
            ErrorCode::NotADatabase => "SQLite3::NotADatabaseException",
            _ => "SQLite3::SQLException",
        },
        SqlInputError { .. } => "SQLite3::SQLException",
        _ => "SQLite3::SQLException",
    };
    (cls.to_string(), msg)
}
