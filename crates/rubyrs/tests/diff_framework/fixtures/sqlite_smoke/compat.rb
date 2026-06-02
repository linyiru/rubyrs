# Runtime-aware loader + thin compat helpers for SQLite3 surface
# differences between rubyrs's `_sqlite` battery and the CRuby
# `sqlite3` gem. Two documented shape mismatches that this shim
# normalises so the fixture's stdout can be byte-diffed:
#
#  1. **execute/query split.** rubyrs's `SQLite3::Database#execute`
#     handles NON-SELECT statements only; SELECT goes through
#     `#query` (returns Array-of-Arrays). The CRuby gem unifies on
#     `#execute` for both shapes. `select(...)` below routes to the
#     right side; `exec_dml(...)` routes the non-SELECT path.
#
#  2. **Params splat vs Array.** rubyrs is
#     `execute(sql, *params)` / `query(sql, *params)` — positional
#     splat. CRuby's gem is `execute(sql, [params])` — single Array
#     argument. The compat helpers take `*params` and re-shape per
#     runtime.
#
# Both surfaces converge again at `prepare` → `Statement`; the
# `stmt_query(...)` helper handles the one residual difference
# (`stmt.execute` on CRuby returns a `ResultSet` you `.to_a` to get
# rows; rubyrs `stmt.query` returns rows directly).
#
# Class names (`SQLite3::Database`, `SQLite3::Statement`,
# `SQLite3::ConstraintException`) match on both sides — the
# 25-class hierarchy in `preamble/sqlite_database.rb` was
# specifically modelled on the CRuby gem's names so user-facing
# `rescue` clauses port one-to-one.
if defined?(RUBYRS)
  require "rubyrs/sqlite"
else
  require "sqlite3"
end

module SQLiteCompat
  # `def self.foo` rather than `module_function` — rubyrs doesn't
  # implement `module_function` as a Module-level method-visibility
  # toggle yet (the metaclass would also need a parallel def
  # synthesised; outside the Tier-1 subset). Same call-site shape
  # `SQLiteCompat.foo(...)` works on both CRuby and rubyrs because
  # `def self.foo` lands the method on the module's singleton class
  # on both.

  # Non-SELECT (INSERT / UPDATE / DELETE / CREATE / DROP / PRAGMA).
  # Return value intentionally discarded — the two runtimes
  # return different no-op shapes (nil vs []) for non-SELECT.
  def self.exec_dml(db, sql, *params)
    if defined?(RUBYRS)
      db.execute(sql, *params)
    else
      db.execute(sql, params)
    end
    nil
  end

  # SELECT — returns Array-of-Arrays on both sides.
  def self.select(db, sql, *params)
    if defined?(RUBYRS)
      db.query(sql, *params)
    else
      db.execute(sql, params)
    end
  end

  # Prepared-statement execute. rubyrs `stmt.query(*params)` returns
  # rows directly; CRuby `stmt.execute(*params)` returns a ResultSet
  # that needs `.to_a`. Both end up Array-of-Arrays.
  def self.stmt_select(stmt, *params)
    if defined?(RUBYRS)
      stmt.query(*params)
    else
      stmt.execute(*params).to_a
    end
  end
end
