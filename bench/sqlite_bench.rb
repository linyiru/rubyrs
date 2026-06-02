# SQLite benchmark — `_sqlite` battery (rubyrs) vs the CRuby
# `sqlite3` gem. Same shape as bench/json_bench.rb: per-iter µs
# via min-of-runs, three workloads (insert / select-one / select-many),
# runtime-aware compat shim so the SAME script runs on both.
#
# Run:
#   ruby                          bench/sqlite_bench.rb       # CRuby + sqlite3 gem
#   target/release/rubyrs         bench/sqlite_bench.rb       # rubyrs Phase 3 battery
#
# Build rubyrs first:
#   cargo build --release --features _sqlite,stdlib -p rubyrs
#
# Compares apples-to-apples on the deterministic subset:
#   - DB is `:memory:` (no FS variance)
#   - schema seeded ONCE per script, before the timed loops
#   - ITERS rows inserted in a single transaction (amortised commit)
#   - Selects re-use one cached prepared statement (cache hit case)
#   - Uncached selects re-prepare each iter (cache miss case — common
#     when SQL is interpolated per call)
#
# Environment knobs (defaults match bench/json_bench.rb):
#   ITERS=2000  RUNS=3

# ---- Runtime-aware load shim ----
# rubyrs's battery uses `require "rubyrs/sqlite"` per ADR 0019
# Rule 8 (avoids shadowing the MRI gem). CRuby's gem is loaded
# the canonical way. Both expose `SQLite3::Database` with the
# same `.new(path)` / `.execute(sql, *params)` /
# `.transaction { ... }` / `.close` surface.
if defined?(RUBYRS)
  require "rubyrs/sqlite"
  RUNTIME_LABEL = "rubyrs (_sqlite battery, Phase 3)"
else
  require "sqlite3"
  RUNTIME_LABEL = "CRuby + sqlite3 gem #{SQLite3::VERSION} (libsqlite3 #{SQLite3::SQLITE_VERSION})"
end

ITERS = Integer(ENV["ITERS"] || "2000")
RUNS  = Integer(ENV["RUNS"]  || "3")

puts "runtime: #{RUNTIME_LABEL}"
puts "iters:   #{ITERS}  runs: #{RUNS}"
puts ""

# ---- Time + bench harness (same shape as json_bench) ----
def time_ms
  t0 = Time.now
  yield
  ((Time.now - t0) * 1000.0).to_f
end

def bench(label, runs, iters)
  best = nil
  runs.times do
    ms = time_ms { iters.times { yield } }
    best = ms if best.nil? || ms < best
  end
  per_iter_us = (best * 1000.0) / iters
  line = sprintf("%-22s  best_total=%9.2f ms   per_iter=%9.3f us",
    label, best, per_iter_us)
  puts line
end

# ---- Schema seed (once, untimed) ----
db = SQLite3::Database.new(":memory:")
db.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL)")

# Pre-load ITERS rows so the SELECT workloads have something to
# read. INSERT itself is benched separately on a fresh table.
# `db.execute(sql, *params)` (rubyrs splat) vs `db.execute(sql,
# [params])` (CRuby array-only) — branch on runtime.
ITERS.times do |i|
  if defined?(RUBYRS)
    db.execute("INSERT INTO users (name, score) VALUES (?, ?)", "user#{i}", i.to_f / 7.0)
  else
    db.execute("INSERT INTO users (name, score) VALUES (?, ?)", ["user#{i}", i.to_f / 7.0])
  end
end

# ---- Workload 1: bulk_insert (transaction-wrapped) ----
# CRuby gem and our battery both auto-commit per-execute by
# default. Wrapping ITERS inserts in one transaction is the
# realistic shape for migrations / bulk seed loads. Reset the
# table between runs so each measurement starts from the same
# baseline.
bench("bulk_insert", RUNS, ITERS) do
  if defined?(RUBYRS)
    db.execute("INSERT INTO users (name, score) VALUES (?, ?)", "u", 1.0)
  else
    db.execute("INSERT INTO users (name, score) VALUES (?, ?)", ["u", 1.0])
  end
end

# Clean up before next workload (count check). Use `query` /
# `execute` per runtime since rubyrs splits the two methods
# (execute = non-SELECT) while CRuby's gem unifies.
total_after_insert = if defined?(RUBYRS)
  db.query("SELECT COUNT(*) FROM users")
else
  db.execute("SELECT COUNT(*) FROM users")
end
puts "rows after bulk_insert phase: #{total_after_insert.first.first}"

# Pre-generate the lookup id sequence ONCE (outside the bench
# loop) so the random-number cost is amortised and isn't part
# of the timed inner block. Plain index sequence — the bench
# wants to exercise prepare-and-bind cost, not RNG. rubyrs
# doesn't yet ship `Array.new(N) { block }`, so use map over
# a range.
LOOKUP_IDS = (0...ITERS).map { |i| (i * 1009 + 17) % ITERS + 1 }

# ---- Workload 2: select_one (cached statement) ----
# Cached path — rubyrs uses `query_cached` (per-conn LRU);
# CRuby uses `db.prepare` once outside the loop + `.execute`
# per iter on the cached statement object. Both shapes hit the
# "already-prepared, just bind + step" cost.
stmt = db.prepare("SELECT name FROM users WHERE id = ?")
i = 0
bench("select_one_cached", RUNS, ITERS) do
  if defined?(RUBYRS)
    stmt.query(LOOKUP_IDS[i % ITERS])
  else
    stmt.execute(LOOKUP_IDS[i % ITERS]).to_a
  end
  i += 1
end
stmt.close

# ---- Workload 3: select_one_uncached ----
# Fresh prepare per iter. Realistic shape when SQL is
# interpolated. CRuby's `execute` accepts array-form params
# OR a single positional; we use array-form for parity.
j = 0
bench("select_one_uncached", RUNS, ITERS) do
  if defined?(RUBYRS)
    db.query("SELECT name FROM users WHERE id = ?", LOOKUP_IDS[j % ITERS])
  else
    db.execute("SELECT name FROM users WHERE id = ?", [LOOKUP_IDS[j % ITERS]])
  end
  j += 1
end

# ---- Workload 4: select_many (full result materialisation) ----
# Pull ALL rows in a single statement — exercises the row-array
# build path, the heap-cap codepath (capped at nil so it doesn't
# trigger), and the Hash-or-Array marshalling.
bench("select_many", RUNS, 100) do
  if defined?(RUBYRS)
    db.query("SELECT id, name, score FROM users")
  else
    db.execute("SELECT id, name, score FROM users")
  end
end

if defined?(RUBYRS)
  puts ""
  puts "cache_hits=#{db.statement_cache_hits} cache_misses=#{db.statement_cache_misses}"
end

db.close
