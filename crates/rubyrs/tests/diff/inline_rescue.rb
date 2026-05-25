# Inline rescue modifier — `expr rescue fallback`. Desugars to
# `begin; expr; rescue StandardError; fallback; end`. Catches only
# StandardError and its subclasses, matching CRuby's contract.

# Basic: a method that raises gets caught.
def boom
  raise "kaboom"
end

x = boom rescue "saved"
puts x

# Same expression evaluated without an exception falls through.
y = "ok" rescue "fail"
puts y

# Math expression that raises ZeroDivisionError.
quot = (1 / 0) rescue -1
puts quot

# Modifier with a method call as the rescued expression.
def echo(x)
  x
end
puts echo("hi") rescue "miss"

# Chains with method calls.
result = boom.upcase rescue "no_upcase"
puts result

# Inline rescue inside an assignment.
val = (raise "x") rescue 42
puts val

# Returns nil if fallback evaluates to nil.
none = boom rescue nil
puts none.inspect

# Rescue scope: only StandardError caught, not bare Exception.
class CustomEx < StandardError; end

def bad
  raise CustomEx, "custom"
end

caught = bad rescue "ok"
puts caught

# Inline rescue with a fallback that itself does work.
def attempt(value)
  value.length
end

# String#length works
puts attempt("hello") rescue 0
# nil.length raises → fallback wins
puts attempt(nil) rescue 0

# Used to handle nil-deref via rescue (canonical CRuby idiom).
def safe_len(s)
  s.length rescue 0
end

puts safe_len("hello")
puts safe_len(nil)

# Inside a method body, after other statements.
def look_up(k)
  hash = {"a" => 1, "b" => 2}
  hash.fetch(k) rescue -1
end

puts look_up("a")
puts look_up("b")
puts look_up("missing")

# Inline rescue in an iterator predicate.
[1, 2, 0, 4].each do |d|
  puts (100 / d) rescue "skip"
end

# Right-associative chain — `a rescue b rescue c` parses as
# `a rescue (b rescue c)`. If the second also raises, the third
# catches.
def twice_bad
  raise "first" rescue raise "second"
end

# In CRuby this would raise "second" — the inner rescue catches
# the first, then re-raises the second; the surrounding handler
# (none, here) doesn't catch it. We re-test with a wrapper.
out = (twice_bad rescue "wrapper-caught")
puts out
