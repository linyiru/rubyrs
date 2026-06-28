# `SQLite3::Database` + the 25-class exception hierarchy per
# ADR 0027 §6. Loaded by `register_host_fns` at battery init
# time so the constants exist before any user script calls
# `require "rubyrs/sqlite"`. The require itself is a lenient
# stub (the is_stdlib_stub_name whitelist accepts
# "rubyrs/sqlite") that succeeds without reloading — the
# classes below are already in place.
#
# Style: empty subclasses are 1-liners; the wrapper class
# methods are thin Ruby that dispatch to `__rubyrs_sqlite_*`
# host fns. No business logic here that isn't either
# Ruby-canonical (the `transaction` block-form) or
# infrastructure (param-pack normalisation).

module SQLite3
  # Top of the hierarchy. Subclasses match the CRuby `sqlite3`
  # gem's `SQLite3::Errors` module so `rescue` clauses port.
  class Exception < StandardError; end

  class SQLException             < Exception; end
  class InternalException        < Exception; end
  class PermissionException      < Exception; end
  class AbortException           < Exception; end
  class BusyException            < Exception; end
  class LockedException          < Exception; end
  class MemoryException          < Exception; end
  class ReadOnlyException        < Exception; end
  class InterruptException       < Exception; end
  class IOException              < Exception; end
  class CorruptException         < Exception; end
  class NotFoundException        < Exception; end
  class FullException            < Exception; end
  class CantOpenException        < Exception; end
  class ProtocolException        < Exception; end
  class EmptyException           < Exception; end
  class SchemaChangedException   < Exception; end
  class TooBigException          < Exception; end
  class ConstraintException      < Exception; end
  class MismatchException        < Exception; end
  class MisuseException          < Exception; end
  class UnsupportedException     < Exception; end
  class AuthorizationException   < Exception; end
  class FormatException          < Exception; end
  class RangeException           < Exception; end
  class NotADatabaseException    < Exception; end

  class Database
    # `SQLite3::Database.quote(str)` — SQL string-literal escaping (double
    # the single quotes). ActiveRecord's sqlite3 adapter calls
    # `@connection.class.quote(s)` from `quote_string`.
    def self.quote(string)
      string.to_s.gsub("'", "''")
    end

    # `db = SQLite3::Database.new("app.db")` opens a connection.
    # `:memory:` (literal) opens an anonymous in-memory DB —
    # convenient for tests + the Sequel-lite fixture in Phase 6.
    #
    # Options Hash accepts:
    #   :busy_timeout_ms (default 5000) — see ADR 0027 §3
    #   :cache_size      (default 100)  — LRU cap, ADR 0027 §4
    def initialize(path, opts = nil)
      @handle = if opts.nil?
        __rubyrs_sqlite_open(path)
      else
        __rubyrs_sqlite_open(path, opts)
      end
      @closed = false
    end

    def close
      return if @closed
      __rubyrs_sqlite_close(@handle)
      @closed = true
      nil
    end

    def closed?
      @closed
    end

    # `db.execute("INSERT INTO users(name) VALUES (?)", name)` —
    # returns rows-changed (Integer). Use for DDL / INSERT /
    # UPDATE / DELETE; for SELECT use `query` instead.
    #
    # This path bypasses the prepared-statement LRU. Use
    # `execute_cached` for hot-loop SQL strings the caller knows
    # are reused. ADR 0027 §4 documents the footgun.
    def execute(sql, *params)
      raise SQLite3::Exception, "closed database" if @closed
      __rubyrs_sqlite_execute(@handle, sql, params)
    end

    # Same shape as `execute` but uses the per-connection LRU
    # (key = SQL string). Hot loops with a SQL literal hit the
    # cache; calls with interpolated SQL still bypass.
    def execute_cached(sql, *params)
      raise SQLite3::Exception, "closed database" if @closed
      __rubyrs_sqlite_execute_cached(@handle, sql, params)
    end

    # `db.query(sql, *params)` — returns Array<Array<Value>>,
    # one inner Array per row, columns in declaration order.
    # Caller `zip`s column names if needed; the Sequel-lite DSL
    # (Phase 5b) handles that. Materialises the full result set;
    # see Config::sqlite_max_result_bytes for the heap-cap gate.
    def query(sql, *params)
      raise SQLite3::Exception, "closed database" if @closed
      __rubyrs_sqlite_query(@handle, sql, params)
    end

    def query_cached(sql, *params)
      raise SQLite3::Exception, "closed database" if @closed
      __rubyrs_sqlite_query_cached(@handle, sql, params)
    end

    # `db.transaction { db.execute(...) }` — block-form transaction
    # with auto-rollback on Ruby exception. ADR 0027 §3.
    #
    # Normal exit → COMMIT; raised Ruby → ROLLBACK + re-raise.
    # Nested calls are outer-only in v1 — inner block executes
    # inside the outer transaction's frame, no SAVEPOINT.
    # SAVEPOINT-nesting is a Tier-B follow-up.
    def transaction(_mode = nil)
      raise SQLite3::Exception, "closed database" if @closed
      # No-block form: open a transaction and return; the caller drives
      # `commit` / `rollback` manually. ActiveRecord's `begin_db_transaction`
      # calls `@raw_connection.transaction` (no block) then commit/rollback
      # in separate adapter methods.
      unless block_given?
        execute("BEGIN") unless @in_transaction
        @in_transaction = true
        return true
      end
      if @in_transaction
        # Nested — just run the body inline.
        return yield
      end
      execute("BEGIN")
      @in_transaction = true
      begin
        result = yield
        execute("COMMIT")
        @in_transaction = false
        result
      rescue => e
        @in_transaction = false
        begin
          execute("ROLLBACK")
        rescue
          # ROLLBACK can itself fail (e.g. broken connection);
          # don't mask the original exception.
        end
        raise e
      end
    end

    # Manual transaction control (the block-less `transaction` companion).
    def commit
      raise SQLite3::Exception, "closed database" if @closed
      execute("COMMIT") if @in_transaction
      @in_transaction = false
      true
    end

    def rollback
      raise SQLite3::Exception, "closed database" if @closed
      execute("ROLLBACK") if @in_transaction
      @in_transaction = false
      true
    end

    # `db.last_insert_row_id` — rowid of the most recent INSERT on this
    # connection (sqlite3 gem API; ActiveRecord's `last_inserted_id` calls
    # it). Equivalent to the C `sqlite3_last_insert_rowid`, surfaced via the
    # SQL function on the same connection.
    def last_insert_row_id
      raise SQLite3::Exception, "closed database" if @closed
      rows = query("SELECT last_insert_rowid()")
      rows.first && rows.first.first
    end

    # `db.changes` — rows affected by the most recent INSERT/UPDATE/DELETE
    # (ActiveRecord reads it for affected-row counts).
    def changes
      raise SQLite3::Exception, "closed database" if @closed
      rows = query("SELECT changes()")
      (rows.first && rows.first.first) || 0
    end

    def busy_timeout=(ms)
      raise SQLite3::Exception, "closed database" if @closed
      __rubyrs_sqlite_busy_timeout(@handle, ms)
    end

    def statement_cache_hits
      raise SQLite3::Exception, "closed database" if @closed
      __rubyrs_sqlite_cache_hits(@handle)
    end

    def statement_cache_misses
      raise SQLite3::Exception, "closed database" if @closed
      __rubyrs_sqlite_cache_misses(@handle)
    end

    # `db.prepare(sql) → SQLite3::Statement` — returns a Ruby-
    # visible prepared statement object the caller holds across
    # iterations. Mirrors the CRuby sqlite3 gem's pattern;
    # closes the `select_one_cached` bench gap by skipping the
    # SQL-string → LRU lookup each `execute` does (the
    # Statement holds its own opaque handle into a separate
    # per-thread map). Always close the returned statement
    # before closing the Database — Database#close auto-sweeps
    # outstanding statements as a safety net, but explicit
    # closes keep the map small.
    def prepare(sql)
      raise SQLite3::Exception, "closed database" if @closed
      Statement.new(self, sql)
    end
  end

  # Prepared statement — bind + step against a precompiled SQL
  # string. Mirrors CRuby's `SQLite3::Statement` so user code
  # ports unchanged. Created via `db.prepare(sql)`; the
  # constructor calls the host fn once to prepare and stash the
  # handle. Subsequent `execute(*params)` / `query(*params)`
  # calls go straight to bind + step (no per-call SQL hashing).
  class Statement
    def initialize(db, sql)
      @db_handle = db.instance_variable_get(:@handle)
      @handle = __rubyrs_sqlite_prepare(@db_handle, sql)
      @closed = false
    end

    # `stmt.execute(*params)` — bind params, step once, return
    # rows-affected. For SELECT use `query` instead (`execute`
    # uses the raw_execute path which errors on result-returning
    # statements, matching the Database method's split).
    def execute(*params)
      raise SQLite3::Exception, "closed statement" if @closed
      __rubyrs_sqlite_stmt_execute(@handle, params)
    end

    # `stmt.query(*params)` — bind params, step through rows,
    # return `Array<Array<Value>>` (one inner array per row).
    def query(*params)
      raise SQLite3::Exception, "closed statement" if @closed
      __rubyrs_sqlite_stmt_query(@handle, params)
    end

    # `stmt.columns` — column names of the prepared statement, in
    # declaration order. ActiveRecord's sqlite3 adapter calls this before
    # stepping (`cols = stmt.columns`) to build the result's column set.
    def columns
      raise SQLite3::Exception, "closed statement" if @closed
      __rubyrs_sqlite_stmt_columns(@handle)
    end

    # `stmt.bind_params(*params)` — stash positional binds for the next
    # `to_a` (the host `query` op binds + steps in one shot). Accepts a
    # single Array (AR's `stmt.bind_params(type_casted_binds)`) or varargs.
    def bind_params(*params)
      params = params.first if params.length == 1 && params.first.is_a?(Array)
      @bound_params = params
      self
    end

    # `stmt.to_a` — step through all rows with the params bound by the most
    # recent `bind_params`, returning `Array<Array<Value>>`.
    def to_a
      raise SQLite3::Exception, "closed statement" if @closed
      __rubyrs_sqlite_stmt_query(@handle, @bound_params || [])
    end

    # `stmt.reset!` — clear bound params so the statement can be re-bound
    # and re-stepped (AR's prepared-statement-cache path). The host `query`
    # op clears bindings on each call, so this just drops our stash.
    def reset!
      @bound_params = nil
      self
    end

    def close
      return if @closed
      __rubyrs_sqlite_stmt_close(@handle)
      @closed = true
      nil
    end

    def closed?
      @closed
    end
  end
end

# `require "rubyrs/sqlite"` resolves to a no-op stub via the
# is_stdlib_stub_name whitelist (the classes above are already
# defined). The bare `require "sqlite3"` shape is INTENTIONALLY
# unsupported — per ADR 0019 Rule 8, the Ruby-side load path for
# native batteries is `rubyrs/<name>` so the MRI `sqlite3` gem
# stays loadable independently when Tier-4 compat lands.
