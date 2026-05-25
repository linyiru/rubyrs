# Full Ruby semantics for `break` / `next` walking through an
# `ensure` chain. Replaces the previous defensive-trap fixtures
# now that proper semantics are wired up (LoopTransfer state on
# the Vm; Op::EndEnsure at the tail of every ensure handler body
# resumes the walk; user `raise` inside an ensure clears the
# pending transfer so the raise wins).
#
# CRuby is the oracle; every scenario must print byte-identical.

# 1. break through single ensure — ensure body runs AND the loop
#    exits cleanly with the break value.
result = nil
v = while true
  begin
    break 42
  ensure
    result = "ran"
  end
end
puts "1: result=#{result.inspect} v=#{v.inspect}"

# 2. next through ensure — ensure runs, iteration is abandoned,
#    loop re-checks cond and continues.
log = []
i = 0
while i < 3
  i = i + 1
  begin
    next if i == 2
  ensure
    log << "ensure-#{i}"
  end
  log << "body-#{i}"
end
puts "2: log=#{log.inspect}"

# 3. Nested ensures — innermost runs first, outermost last
#    (LIFO), all before the break lands.
trail = []
v3 = while true
  begin
    begin
      break "deep"
    ensure
      trail << "inner"
    end
  ensure
    trail << "outer"
  end
end
puts "3: trail=#{trail.inspect} v=#{v3.inspect}"

# 4. raise inside ensure during a pending break — the raise wins
#    (CRuby: the break is silently dropped, the exception
#    propagates). Verified by the outer rescue catching the
#    secondary exception and the break's loop-exit never
#    completing on its terms.
catch_ran = false
caught_msg = nil
begin
  while true
    begin
      break 42
    ensure
      raise "secondary"
    end
  end
rescue => e
  catch_ran = true
  caught_msg = e.message
end
puts "4: catch_ran=#{catch_ran} caught=#{caught_msg.inspect}"

# 5. break outside ensure still works — no behaviour change for
#    the common path that doesn't involve an ensure.
v5 = while true
  break 99
end
puts "5: v=#{v5.inspect}"

# 6. ensure runs even when break value is computed from a
#    side-effecting expression — verifies the value is captured
#    before the transfer starts.
side_effects = []
v6 = while true
  begin
    side_effects << "before-break"
    break "value-#{side_effects.length}"
  ensure
    side_effects << "in-ensure"
  end
end
puts "6: effects=#{side_effects.inspect} v=#{v6.inspect}"

# 7. next through ensure, with the `next val` form. CRuby
#    discards the value (while has no iteration-value channel)
#    but the expression evaluates for its side effects, and
#    THEN the ensure runs.
trail7 = []
j = 0
while j < 3
  j = j + 1
  begin
    next trail7 << "next-arg-#{j}" if j == 2
  ensure
    trail7 << "ensure-#{j}"
  end
  trail7 << "body-#{j}"
end
puts "7: trail=#{trail7.inspect}"
