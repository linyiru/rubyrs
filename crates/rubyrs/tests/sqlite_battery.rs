//! Integration tests for the `_sqlite` battery (ADR 0027).
//!
//! Driven via the rubyrs binary so we exercise the same
//! Ruby↔host-fn boundary user scripts hit. Uses `:memory:` DBs
//! so the suite has zero FS / journal-mode variance.
//!
//! Two test classes:
//!   - `Database#*` shape: open / execute / query / transaction
//!     (incl. rollback on exception) / 25-class exception
//!     hierarchy matching.
//!   - `Statement#*` shape (Phase 3.1): prepare-once /
//!     `stmt.execute` / `stmt.query` / `stmt.close` /
//!     auto-orphan-sweep on `Database#close`.

#![cfg(feature = "_sqlite")]

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Drive a Ruby script via the rubyrs binary; return stdout.
/// Asserts non-zero exit status on stderr for visibility.
fn run_rubyrs(script: &str, fixture_name: &str) -> String {
    let driver_dir = env!("CARGO_TARGET_TMPDIR");
    let driver = PathBuf::from(driver_dir).join(format!("sqlite_battery_{fixture_name}.rb"));
    fs::write(&driver, script).expect("failed to write driver.rb");
    let rubyrs_bin = env!("CARGO_BIN_EXE_rubyrs");
    let out = Command::new(rubyrs_bin)
        .arg(&driver)
        .output()
        .expect("failed to spawn rubyrs binary");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        out.status.success(),
        "rubyrs exited non-zero for {fixture_name}:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    stdout
}

#[test]
fn database_open_execute_query_roundtrip() {
    let script = r#"
require "rubyrs/sqlite"

db = SQLite3::Database.new(":memory:")
db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, score REAL)")
db.execute("INSERT INTO t (name, score) VALUES (?, ?)", "alice", 1.5)
db.execute("INSERT INTO t (name, score) VALUES (?, ?)", "bob", 2.0)

rows = db.query("SELECT id, name, score FROM t ORDER BY id")
rows.each { |r| puts r.inspect }
db.close
"#;
    let stdout = run_rubyrs(script, "roundtrip");
    assert_eq!(stdout, "[1, \"alice\", 1.5]\n[2, \"bob\", 2.0]\n");
}

#[test]
fn database_transaction_commit_on_normal_exit() {
    let script = r#"
require "rubyrs/sqlite"
db = SQLite3::Database.new(":memory:")
db.execute("CREATE TABLE t (id INTEGER)")
db.transaction do
  db.execute("INSERT INTO t VALUES (?)", 1)
  db.execute("INSERT INTO t VALUES (?)", 2)
end
puts db.query("SELECT COUNT(*) FROM t").first.first
"#;
    let stdout = run_rubyrs(script, "tx_commit");
    assert_eq!(stdout, "2\n");
}

#[test]
fn database_transaction_rollback_on_exception() {
    let script = r#"
require "rubyrs/sqlite"
db = SQLite3::Database.new(":memory:")
db.execute("CREATE TABLE t (id INTEGER)")
db.execute("INSERT INTO t VALUES (?)", 1)
begin
  db.transaction do
    db.execute("INSERT INTO t VALUES (?)", 2)
    raise "boom"
  end
rescue
end
puts db.query("SELECT COUNT(*) FROM t").first.first
"#;
    let stdout = run_rubyrs(script, "tx_rollback");
    assert_eq!(stdout, "1\n");
}

#[test]
fn constraint_exception_caught_via_named_subclass() {
    // SQLite3::ConstraintException is one of the 25-class
    // hierarchy. Confirms the trap_to_exception path picks up
    // nested module classes by qualified sym (vm/raise.rs).
    let script = r#"
require "rubyrs/sqlite"
db = SQLite3::Database.new(":memory:")
db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)")
db.execute("INSERT INTO t (id) VALUES (1)")
begin
  db.execute("INSERT INTO t (id) VALUES (1)")
rescue SQLite3::ConstraintException => e
  puts "caught:" + e.class.name
end
"#;
    let stdout = run_rubyrs(script, "constraint");
    assert_eq!(stdout, "caught:SQLite3::ConstraintException\n");
}

#[test]
fn statement_prepare_query_execute_close() {
    // Phase 3.1 — SQLite3::Statement Ruby class.
    let script = r#"
require "rubyrs/sqlite"
db = SQLite3::Database.new(":memory:")
db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT)")

# Statement-based INSERT (execute returns rows-changed)
ins = db.prepare("INSERT INTO t (name) VALUES (?)")
puts ins.execute("alice")
puts ins.execute("bob")
ins.close

# Statement-based SELECT (query returns rows)
sel = db.prepare("SELECT name FROM t WHERE id = ?")
puts sel.query(1).inspect
puts sel.query(2).inspect
puts sel.query(99).inspect    # empty result
sel.close

puts db.query("SELECT COUNT(*) FROM t").first.first
"#;
    let stdout = run_rubyrs(script, "stmt_roundtrip");
    assert_eq!(stdout, "1\n1\n[[\"alice\"]]\n[[\"bob\"]]\n[]\n2\n");
}

#[test]
fn statement_orphans_when_database_closes() {
    // db.close sweeps outstanding statements before dropping
    // the Connection (sqlite.rs's STMT_HANDLES `retain` filter
    // on owner_handle). A subsequent stmt call on the orphan
    // returns SQLite3::Exception with a "closed statement"
    // message rather than UB on the dangling Statement<'static>.
    let script = r#"
require "rubyrs/sqlite"
db = SQLite3::Database.new(":memory:")
db.execute("CREATE TABLE t (id INTEGER PRIMARY KEY)")
stmt = db.prepare("SELECT * FROM t WHERE id = ?")
db.close
begin
  stmt.query(1)
rescue SQLite3::Exception => e
  puts "rescued:" + e.class.name
end
"#;
    let stdout = run_rubyrs(script, "stmt_orphan");
    assert_eq!(stdout, "rescued:SQLite3::Exception\n");
}

#[test]
fn statement_close_idempotent() {
    let script = r#"
require "rubyrs/sqlite"
db = SQLite3::Database.new(":memory:")
db.execute("CREATE TABLE t (id INTEGER)")
stmt = db.prepare("SELECT * FROM t")
puts stmt.closed?
stmt.close
puts stmt.closed?
stmt.close
puts stmt.closed?
"#;
    let stdout = run_rubyrs(script, "stmt_close_idempotent");
    assert_eq!(stdout, "false\ntrue\ntrue\n");
}
