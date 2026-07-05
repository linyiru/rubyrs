# A pattern that PARSES (fancy-regex's syntax-only parse_tree accepts
# it) but fails the real engine BUILD (e.g. an invalid char-class
# range) surfaces as RegexpError. CRuby raises at Regexp.new; rubyrs's
# lazy-compile posture raises the SAME class at FIRST USE — both land
# inside the begin/rescue below, so the printed output is identical.
# Fuzz find 2026-07: rubyrs used to PANIC at first match
# (`regex_engine.rs` deferred-build `panic!`, introduced by the
# defer-all-validated-patterns perf change).
def check(label)
  yield
  puts "#{label}: no-raise"
rescue RegexpError => e
  puts "#{label}: #{e.class} std=#{e.is_a?(StandardError)}"
rescue => e
  puts "#{label}: OTHER #{e.class}"
end

# The fuzzer's minimal repro shape (String#start_with? was the first
# toucher of the deferred build).
check("start_with?") { "x".start_with?(Regexp.new("[a-#b c dz]")) }

# parse_tree-accepts-but-build-rejects battery, construct + first
# match inside one rescue.
["[a-#b c dz]", "[z-a]", "(a)\\2", "a{100000000}"].each do |pat|
  check("match #{pat.inspect}") { Regexp.new(pat) =~ "abc" }
end

# Interpolated regex literal — CRuby also compiles this at runtime,
# so both implementations raise a rescuable RegexpError here.
bad = "[z-a]"
check("interp-literal") { /#{bad}/ =~ "abc" }

# First use INSIDE a method, rescued at the caller.
def first_use_in_method(re)
  re.match?("abc")
end
begin
  first_use_in_method(Regexp.new("[z-a]"))
  puts "method: no-raise"
rescue RegexpError
  puts "method: caught #{$!.class}"
end

# Bare rescue (StandardError) catches it — the RuboCop per-cop
# error-handler contract (same as regexp_error_standarderror.rb).
result = begin
  Regexp.new("[a-#b c dz]") =~ "abc"
  "no-raise"
rescue
  "bare-rescue-caught #{$!.class}"
end
p result

# The cached build failure raises AGAIN on every subsequent use —
# it must not degrade to "no match" after the first raise. CRuby
# can't construct the object at all (rescue → nil; the manufactured
# raise keeps the printed output aligned); the rubyrs side reuses
# ONE object so the cached-Err path is exercised twice.
re2 = (Regexp.new("[z-a]") rescue nil)
2.times do |i|
  check("use-#{i}") { re2 ? re2 =~ "abc" : raise(RegexpError, "unconstructible") }
end

# Non-matching first touchers: reflection and the other operation
# families each surface the same error.
check("names")     { Regexp.new("[z-a]").names }
check("named_cap") { Regexp.new("[z-a]").named_captures }
check("split")     { "a,b".split(Regexp.new("[z-a]")) }
check("sub")       { "abc".sub(Regexp.new("[z-a]"), "x") }
check("gsub-blk")  { "abc".gsub(Regexp.new("[z-a]")) { "x" } }
check("scan")      { "abc".scan(Regexp.new("[z-a]")) }
check("case-when") { case "abc" when Regexp.new("[z-a]") then 1 else 2 end }
check("partition") { "abc".partition(Regexp.new("[z-a]")) }

# A malformed pattern that ALSO fails the cheap parse gate keeps the
# construction-time raise (eager error, not deferred).
check("eager-new") { Regexp.new("[") }
