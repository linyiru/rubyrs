# rescue-splat filters: `rescue *CONST` (minitest's
# PASSTHROUGH_EXCEPTIONS idiom) and `rescue *local` (minitest's
# assert_raises *exp). Before these forms existed the splat was
# silently dropped and the clause degraded to a bare rescue that
# matched EVERY StandardError — minitest's passthrough arm then
# re-raised every test error and killed the whole run.

# --- const splat, lexically scoped (minitest's exact shape)
module M
  class T
    PASS = [NotImplementedError, NoMemoryError]
    def cap
      yield
    rescue *PASS
      puts "pass-arm"
      raise
    rescue Exception => e
      puts "exc-arm: #{e.class}"
    end
  end
end
t = M::T.new
t.cap { nil.zz }
begin
  t.cap { raise NotImplementedError, "x" }
rescue NotImplementedError
  puts "rethrown ok"
end

# --- top-level const splat + single-class coercion
SINGLE = ArgumentError
def cap2
  yield
rescue *SINGLE
  puts "single-arm"
rescue Exception
  puts "other-arm"
end
cap2 { raise ArgumentError }
cap2 { raise TypeError }

# --- local splat (assert_raises shape), incl. bind + default
def assert_raises_like(*exp)
  exp << StandardError if exp.empty?
  begin
    yield
  rescue *exp => e
    puts "matched: #{e.class}"
    return e
  rescue Exception => e
    puts "wrong-class: #{e.class}"
  end
  puts "nothing raised"
end
assert_raises_like(ArgumentError) { raise ArgumentError }
assert_raises_like(ArgumentError) { raise TypeError }
assert_raises_like { raise "boom" }
assert_raises_like(ArgumentError, TypeError) { raise TypeError }
assert_raises_like(ArgumentError) { }

# --- splat alongside literal classes in sibling clauses
ERRS = [ZeroDivisionError]
def cap3
  yield
rescue TypeError
  puts "literal-arm"
rescue *ERRS
  puts "splat-arm"
end
cap3 { raise TypeError }
cap3 { raise ZeroDivisionError }
puts "done"
