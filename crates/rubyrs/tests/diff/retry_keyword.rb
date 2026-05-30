## `retry` — re-executes the surrounding `begin` block from the
## start, re-evaluating the rescue clauses. Only legal inside a
## `rescue` clause body. CRuby raises SyntaxError at parse time
## when used outside; rubyrs catches the out-of-context case at
## compile time and raises a RuntimeError at runtime instead —
## a documented Tier-1 divergence on the error class for an
## error-only path.
##
## Discovery context: rackup-2.2.1/lib/rackup/server.rb:439 uses
## the canonical retry-on-EADDRINUSE pattern; sinatra-4
## transitively requires rackup, so loading `sinatra/base`
## tripped on this. (TRY_RUNS pass-10 layer #9.)

## Shape 1: canonical retry-with-bounded-attempts pattern.
attempts = 0
result = begin
  attempts += 1
  raise "fail" if attempts < 3
  "succeeded"
rescue => e
  retry if attempts < 3
  "gave-up: #{e.message}"
end
puts "result=#{result}"
puts "attempts=#{attempts}"

## Shape 2: retry inside a class-targeted rescue. The class
## filter still matches on each retry — local state (counter)
## is preserved across iterations because retry re-runs the
## begin body, not the surrounding lexical scope.
counter = 0
begin
  counter += 1
  raise ArgumentError, "bad ##{counter}" if counter < 2
  puts "passed-on-attempt=#{counter}"
rescue ArgumentError
  retry if counter < 2
end

## Shape 3: retry inside a nested begin (inner-most rescue
## wins). The inner block retries; outer rescue should never
## fire because the inner one resolves the failure.
outer_runs = 0; inner_runs = 0
begin
  outer_runs += 1
  begin
    inner_runs += 1
    raise "x" if inner_runs < 2
    puts "inner-passed=#{inner_runs}"
  rescue
    retry if inner_runs < 2
  end
rescue
  puts "outer-rescue-fired"
end
puts "outer-runs=#{outer_runs}"

## Shape 4: retry with `ensure` — the ensure body fires on
## EACH iteration. CRuby semantics: ensure runs once per
## iteration completion, regardless of whether the iteration
## succeeded or retried.
$ensure_count = 0
attempts = 0
begin
  begin
    attempts += 1
    raise "e" if attempts < 2
  ensure
    $ensure_count += 1
  end
rescue
  retry if attempts < 2
end
puts "ensure-count=#{$ensure_count}"

## Shape 5: rescue with a binding pattern (typed exception
## bound to a local) still allows retry — the binding is
## re-evaluated on each catch.
seen_classes = []
counter = 0
begin
  counter += 1
  case counter
  when 1 then raise ArgumentError, "first"
  when 2 then raise TypeError, "second"
  else "done-on-#{counter}"
  end
rescue StandardError => e
  seen_classes << e.class.name
  retry if counter < 3
end
puts "seen=#{seen_classes.inspect}"
puts "final-counter=#{counter}"

## Shape 6: multi-class rescue clause `rescue A, B` with
## retry — sibling-class PushRescue entries from the failed
## iteration must NOT accumulate on the rescue stack across
## retries. Pre-fix the unwinder consumed only the matched
## handler (one class), and each retry pushed a fresh pair
## on top of the orphaned sibling, so after the begin block
## completed an outer raise of the OTHER class would be
## (incorrectly) caught by the stale handler — re-entering
## the inner rescue body's IP and re-running the trailing
## code on EACH stale entry. (Code-review #306 round 1.)
acc_counter = 0
begin
  acc_counter += 1
  raise ArgumentError, "boom" if acc_counter < 3
rescue ArgumentError, TypeError
  retry if acc_counter < 3
end
$once_after_begin = 0
$once_after_begin += 1
err = begin
  raise TypeError, "must-propagate"
  "no-raise"
rescue TypeError => e
  "caught-here:#{e.message}"
end
puts "after-begin-count=#{$once_after_begin}"
puts "trailing-rescue=#{err}"
