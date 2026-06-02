# ADR 0027 Phase 4 — `_sqlite` battery parity smoke. Same script
# runs on rubyrs (`require "rubyrs/sqlite"`) and on CRuby (`require
# "sqlite3"`). The compat shim papers over the two documented
# surface differences (execute/query split + params splat vs
# Array) so the per-line stdout below is byte-identical and the
# framework-parity harness can diff it.
#
# Covers the load-bearing surface ADR 0027 promises:
#   1. `Database.new(":memory:")` open
#   2. `execute` non-SELECT (CREATE / INSERT)
#   3. `query`-shaped SELECT — row materialisation
#   4. `prepare` → `Statement` with positional bind
#   5. `transaction { ... }` block-form COMMIT on success
#   6. `transaction { ... raise ... }` block-form ROLLBACK on raise
#   7. `SQLite3::ConstraintException` catch with rescue-class
#      portability across runtimes
#   8. `Statement#close` + `Database#close` idempotent shutdown
require_relative "compat"

db = SQLite3::Database.new(":memory:")

# Schema. UNIQUE on `name` is intentional — exercised by case (7).
db.execute(
  "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, score REAL)"
)
puts "schema_created=true"

# --- 2. Plain non-SELECT inserts with bound params ---
SQLiteCompat.exec_dml(db, "INSERT INTO users (name, score) VALUES (?, ?)", "alice", 3.5)
SQLiteCompat.exec_dml(db, "INSERT INTO users (name, score) VALUES (?, ?)", "bob",   1.25)
puts "inserted=2"

# --- 3. SELECT one + SELECT all ---
row = SQLiteCompat.select(db, "SELECT name, score FROM users WHERE name = ?", "alice").first
puts "alice_row=#{row.inspect}"

all_names = SQLiteCompat.select(db, "SELECT name FROM users ORDER BY id").map { |r| r.first }
puts "all_names=#{all_names.inspect}"

# --- 4. Prepared statement: re-use across two binds ---
stmt = db.prepare("SELECT score FROM users WHERE name = ?")
puts "alice_score_stmt=#{SQLiteCompat.stmt_select(stmt, 'alice').first.first}"
puts "bob_score_stmt=#{SQLiteCompat.stmt_select(stmt, 'bob').first.first}"
stmt.close
puts "stmt_closed=true"

# --- 5. Successful transaction (COMMIT path) ---
db.transaction do
  SQLiteCompat.exec_dml(db, "INSERT INTO users (name, score) VALUES (?, ?)", "carol", 9.9)
end
carol_seen = SQLiteCompat.select(db, "SELECT score FROM users WHERE name = ?", "carol").first.first
puts "carol_committed=#{carol_seen}"

# --- 6. Aborted transaction (ROLLBACK on raise) ---
begin
  db.transaction do
    SQLiteCompat.exec_dml(db, "INSERT INTO users (name, score) VALUES (?, ?)", "dave", 0.5)
    raise "user-error inside txn"
  end
rescue => e
  puts "txn_raised=#{e.message}"
end
dave_count = SQLiteCompat.select(db, "SELECT count(*) FROM users WHERE name = ?", "dave").first.first
puts "dave_count_after_rollback=#{dave_count}"

# --- 7. ConstraintException catch — UNIQUE collision on `name` ---
begin
  SQLiteCompat.exec_dml(db, "INSERT INTO users (name, score) VALUES (?, ?)", "alice", 0.0)
  puts "constraint_unexpectedly_succeeded"
rescue SQLite3::ConstraintException
  puts "constraint_caught=true"
end

# --- 8. Final row count + clean shutdown ---
final_count = SQLiteCompat.select(db, "SELECT count(*) FROM users").first.first
puts "final_count=#{final_count}"

db.close
puts "db_closed=true"
