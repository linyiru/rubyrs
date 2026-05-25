# `next` from inside a `while` loop — skips the rest of the
# current iteration and re-evaluates the loop guard. Distinct
# break-target from `next` inside a block (which signals the
# iteration driver). Scenarios 1-4 cover the core behaviors;
# scenarios 5-7 mirror the break_in_while.rb shape (block-vs-
# while, post-condition form, raise-out-of-inner). Pre-PR rubyrs
# compiled `next` to `Op::Return`, so any of these would silently
# return from the enclosing method or toplevel script.

# 1. Plain next — skip the iteration where i == 3, print the rest.
i = 0
while i < 5
  i = i + 1
  next if i == 3
  puts "1: i=#{i}"
end
puts "1-end: i=#{i}"

# 2. next inside a rescue body inside a while — after the rescue
#    catches, the iteration is abandoned and the loop continues.
j = 0
while j < 5
  j = j + 1
  begin
    raise "skip" if j == 2
  rescue
    next
  end
  puts "2: j=#{j}"
end
puts "2-end: j=#{j}"

# 3. next inside a begin body (no exception fires) with rescue
#    installed — the VM must pop the handler before jumping back
#    to the iter-check, or it leaks across iterations.
k = 0
while k < 5
  k = k + 1
  begin
    next if k == 2
  rescue
    # never enters
  end
  puts "3: k=#{k}"
end
puts "3-end: k=#{k}"

# 4. `next 99` — the value is evaluated for side effects but
#    discarded (while has no iteration-value channel).
side = []
m = 0
while m < 3
  m = m + 1
  next side << m if m == 2
  puts "4: m=#{m}"
end
puts "4-end: side=#{side.inspect}"

# 5. next inside a block inside a while — block-next must NOT
#    skip the while iteration; only the block exits early.
hits = 0
n = 0
while n < 3
  [10, 20, 30].each do |x|
    next if x == 20
    hits = hits + 1
  end
  n = n + 1
end
puts "5: n=#{n} hits=#{hits}"

# 6. Post-condition `begin...end while cond` form. next must
#    jump to the cond evaluation, not body_start.
o = 0
begin
  o = o + 1
  next if o == 2
  puts "6: o=#{o}"
end while o < 4
puts "6-end: o=#{o}"

# 7. raise out of inner while caught by outer-while-surrounding
#    rescue → loop_rescue_depths truncate path. Identical setup
#    to break_in_while scenario #7 but uses `next` to continue.
outer = 0
while outer < 5
  outer = outer + 1
  begin
    while true
      raise "inner-raise"
    end
  rescue => e
    # rescue body
  end
  next if outer == 3
  puts "7: outer=#{outer}"
end
puts "7-end: outer=#{outer}"
