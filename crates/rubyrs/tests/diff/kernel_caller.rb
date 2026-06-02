## `Kernel#caller` — backtrace as Array<String>. Format
## "filename:line:in 'method'" (single quotes, CRuby 3.x).
##
## Discovery: TRY_RUNS pass-12 — sinatra/base.rb:1913
## `cleaned_caller` calls `caller(1)` and splits each line on
## `/:(?=\d|in )/`. Without this builtin, Sinatra::Application <
## Base failed during base.rb's own load (the just-landed
## inherited hook fires it). (Layer #15.)
##
## CRuby qualifies method names as "Object#method" / "Class#method";
## rubyrs's Tier-1 emits just the bare method name. The colon
## layout and the literal `in '...'` are what sinatra's split
## regex matches on, so this divergence is functionally
## equivalent for the parse-and-split use case. Fixture
## normalizes the method-name segment so the diff_cruby
## comparison passes on both interpreters.

def normalize(line)
  # Strip the "Object#" / "Class#" qualifier so rubyrs and
  # CRuby both render the bare method name.
  line.sub(/'(?:[A-Z][\w:]*#)?/, "'")
end

## Shape 1: bare `caller` — array, deepest call first, skips
## the calling method itself? No — skips `caller`'s own frame
## only, so the deepest entry is the calling method's caller.
def s1_inner; caller; end
def s1_outer; s1_inner; end
result = s1_outer
puts "shape1-len=#{result.length}"
puts "shape1=#{result.map { |l| normalize(l) }.inspect}"

## Shape 2: `caller(0)` includes the current calling method's
## frame at the head.
def s2_inner; caller(0); end
def s2_outer; s2_inner; end
result = s2_outer
puts "shape2-len=#{result.length}"
puts "shape2-head=#{normalize(result.first)}"

## Shape 3: `caller(1)` is equivalent to `caller`.
def s3_inner; [caller.length, caller(1).length]; end
def s3_outer; s3_inner; end
puts "shape3-eq=#{s3_outer.inspect}"

## Shape 4: `caller(n, len)` returns up to `len` frames.
def s4_inner; caller(0, 1); end
def s4_outer; s4_inner; end
result = s4_outer
puts "shape4-len=#{result.length}"
puts "shape4-head=#{normalize(result.first)}"

## Shape 5: `caller(huge)` past depth returns nil.
def s5; caller(99); end
puts "shape5=#{s5.inspect}"

## Shape 6: negative arg raises ArgumentError.
err = begin
  caller(-1)
  "no-raise"
rescue ArgumentError => e
  e.message.include?("negative level") ? "negative-level" : "other-ArgError"
end
puts "shape6=#{err}"

## Shape 7: too many args raises ArgumentError.
err = begin
  caller(1, 2, 3)
  "no-raise"
rescue ArgumentError => e
  "arity"
end
puts "shape7=#{err}"

## Shape 8: non-Integer arg raises TypeError (CRuby distinguishes
## arity vs coercion failures here — code that catches one but
## not the other depends on this split).
err = begin
  caller("1")
  "no-raise"
rescue TypeError => e
  e.message.include?("String into Integer") ? "type-string" : "other-TypeError"
end
puts "shape8=#{err}"

err = begin
  caller(nil)
  "no-raise"
rescue TypeError => e
  # CRuby's nil-arg wording: "no implicit conversion from nil
  # to integer" (lowercase 'integer', "from ... to" not "of ...
  # into"). Code-review #342 round 2 caught the original
  # `"nil into Integer"` substring — it never matched, so the
  # test was passing only because BOTH interpreters fell into
  # the `other-TypeError` branch.
  e.message.include?("from nil to integer") ? "type-nil" : "other-TypeError"
end
puts "shape8b=#{err}"
