# `break` from inside a `while` loop — exits to the statement
# immediately after the loop. This is a different break-target
# from `break` inside a block (which signals the iteration
# driver). The four scenarios below were all previously broken
# in rubyrs: `break` compiled to `Op::Return`, so it would
# return from the enclosing method (or the toplevel script),
# instead of jumping past the `while`.

# 1. Plain break — should exit the while and run the trailing
#    `puts`, not silently exit the script.
i = 0
while i < 100
  break if i == 3
  i = i + 1
end
puts "1: i=#{i}"

# 2. break inside a rescue body inside a while — the user-reported
#    case that revealed the bug while writing the msgpack cases
#    test. Loop should exit normally; "after loop" must print.
j = 0
while j < 100
  begin
    raise "stop" if j == 5
    j = j + 1
  rescue
    break
  end
end
puts "2: j=#{j}"

# 3. break inside a begin body (no exception fires) while a rescue
#    handler is still installed — the VM must pop the handler
#    entry before jumping out, or it leaks across the while.
k = 0
while k < 100
  begin
    break if k == 7
    k = k + 1
  rescue
    # never enters
  end
end
puts "3: k=#{k}"

# 4. break N — the loop expression's value should be N. Without
#    the fix, `break 42` would return 42 from the toplevel script.
v = while true
  break 42
end
puts "4: v=#{v.inspect}"

# 5. break inside a block inside a while — block break must NOT
#    exit the while. Each iteration's block exits early; while
#    keeps going.
hits = 0
m = 0
while m < 3
  [10, 20, 30].each do |x|
    hits = hits + 1
    break if x == 20
  end
  m = m + 1
end
puts "5: m=#{m} hits=#{hits}"

# 6. Post-condition `begin...end while cond` form. Reviewer
#    (Angle A) flagged the post arm of the while codegen as
#    untested — same EnterLoop/ExitLoop wrapping but
#    body-then-cond order, so a stack-balance regression there
#    would slip past the pre-form cases.
n = 0
v6 = begin
  n = n + 1
  break "done at #{n}" if n == 3
  "loop"
end while n < 100
puts "6: n=#{n} v=#{v6.inspect}"

# 7. raise out of inner while caught by outer-while-surrounding
#    rescue → the outer's loop_rescue_depths must not retain an
#    orphan entry. Reviewer (Angle A/C) reproduced via a
#    constructed scenario; this minimal version exercises the
#    same path: outer while encloses a begin/rescue that wraps
#    an inner while which raises. After the rescue runs, the
#    outer continues; its `break` must read the outer's depth,
#    not a leaked inner-loop entry. Without the unwind-side
#    truncate this `break` would pop the wrong handler count
#    and the assertion below would diverge.
outer = 0
while outer < 5
  begin
    while true
      raise "inner-raise"
    end
  rescue => e
    # nothing; just consume the exception
  end
  outer = outer + 1
  break if outer == 3
end
puts "7: outer=#{outer}"
