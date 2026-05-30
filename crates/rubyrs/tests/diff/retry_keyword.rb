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

## Shape 7: nested begin where the inner one's `EnterBegin`
## baseline gets bypassed because the inner rescue's filter
## doesn't match — the exception propagates through to the
## outer rescue, which retries. The unwind needs to truncate
## `begin_rescue_depths` to the depth recorded when the outer
## handler was pushed; without it, the inner orphan baseline
## stays at the top and outer-retry's
## TruncateRescuesToBeginBaseline shrinks `rescues` to the
## wrong depth. (Code-review #306 round 2.)
outer_c = 0
nested_result = begin
  outer_c += 1
  begin
    raise ArgumentError, "inner" if outer_c < 3
    "inner-ok-#{outer_c}"
  rescue TypeError
    "wont-match"
  end
rescue ArgumentError
  retry if outer_c < 3
  "outer-fail"
end
puts "nested-result=#{nested_result.inspect}"
puts "nested-outer-c=#{outer_c}"
# Trailing raise — stale baselines shouldn't catch.
trailing = begin
  raise TypeError, "trailing"
rescue TypeError => e
  "caught:#{e.message}"
end
puts "trailing=#{trailing}"

## Shape 8: rescue body that DOESN'T retry but the
## multi-class clause matched one filter — the unwound stack
## might still have the sibling-class entry. The rescue body
## completing without retry must truncate to the begin
## baseline before exiting so a later trailing raise outside
## the begin block isn't caught by the orphan.
## (Code-review #306 round 3.)
n = 0
result_no_retry = begin
  n += 1
  raise ArgumentError, "stop-now"
rescue ArgumentError, TypeError
  "stopped-at-#{n}"
end
puts "no-retry-result=#{result_no_retry}"
post_raise = begin
  raise TypeError, "trailing-no-retry"
rescue TypeError => e
  "caught-here:#{e.message}"
end
puts "post-raise=#{post_raise}"

## Shape 9: while loop inside rescue body, with retry. The
## loop's EnterLoop pushes loop_rescue_depths /
## loop_stack_depths; retry must restore those to the values
## recorded at begin_top time, else a subsequent EnterLoop /
## BreakLoop would read stale entries. (Code-review #306
## round 3.)
c = 0
$visited = []
begin
  c += 1
  raise "x" if c < 3
  $visited << "main-end"
rescue
  3.times do |i|
    $visited << "inner-#{c}-#{i}"
    break if i == 1
  end
  retry if c < 3
end
puts "loop-retry-c=#{c}"
puts "visited=#{$visited.inspect}"
