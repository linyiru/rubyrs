# The other half of the suspended-ensure-walk contract (see
# ensure_walk_contained_raise.rb): when control ESCAPES a suspended
# ensure body — an exception propagating out, a newer
# return/break/next crossing out, a throw to an outer catch — the
# suspended walk is cancelled and the newer control flow wins
# (CRuby). These shapes guard the escape/supersede sweeps.

# 1. raise ESCAPING the ensure body cancels the pending return.
def raise_escapes
  return 1
ensure
  raise "wins"
end
begin
  p raise_escapes
rescue => e
  puts "1: caught #{e.message}"
end

# 2. Escape to an outer rescue in the SAME frame (handler below the
#    suspension baseline).
def escape_same_frame
  begin
    return 2
  ensure
    raise "wins"
  end
rescue => e
  "2: caught-#{e.message}"
end
p escape_same_frame

# 3. Contained rescue that re-raises a DIFFERENT error — the
#    second raise escapes and cancels the return.
def rescue_then_reraise
  return 3
ensure
  begin
    raise "first"
  rescue
    raise "second"
  end
end
begin
  p rescue_then_reraise
rescue => e
  puts "3: caught #{e.message}"
end

# 4. return inside the ensure of a pending return — the newer
#    return supersedes (its value wins).
def return_supersedes
  return :old
ensure
  return :new
end
p return_supersedes

# 5. break-with-ensure inside the ensure of a pending break — the
#    nested transfer completes, then the outer one lands.
r5 = while true
  begin
    break 5
  ensure
    inner = while true
      begin
        break :inner
      ensure
        puts "5: inner-ensure"
      end
    end
    puts "5: inner=#{inner.inspect}"
  end
end
p r5

# 6. next in the ensure of a pending return — the next supersedes;
#    the loop continues and the method falls through.
def next_supersedes
  i = 0
  while i < 2
    i += 1
    begin
      return :returned
    ensure
      next
    end
  end
  puts "6: i=#{i}"
  :fell_through
end
p next_supersedes

# 7. throw in the ensure of a pending return, caught OUTSIDE the
#    body — the throw wins.
def throw_escapes
  catch(:x) do
    begin
      return :returned
    ensure
      throw :x
    end
  end
  :caught
end
p throw_escapes

# 8. return from inside a suspended break-walk's ensure body when
#    the frame has NO further ensure handlers (plain-return frame
#    pop must cancel the stale loop transfer; a later unrelated
#    ensure's EndEnsure must not resume it).
def return_from_break_ensure
  while true
    begin
      break
    ensure
      return 8
    end
  end
  :not_here
end
def later_unrelated_ensure
  begin
    raise "x"
  ensure
    nil
  end
end
p return_from_break_ensure
begin
  later_unrelated_ensure
rescue => e
  puts "8: caught #{e.message}"
end

# 9. Multi-frame non-local return crossing TWO methods' ensures,
#    each with a contained raise on the way down.
def inner9
  [1].each do
    yield
  end
ensure
  begin
    raise "inner-e"
  rescue
    puts "9: inner-rescued"
  end
end
def outer9
  inner9 { return 9 }
  :not_here
ensure
  begin
    raise "outer-e"
  rescue
    puts "9: outer-rescued"
  end
end
p outer9

# 10. Method-level ensure pair: return through both, contained
#     raises in each (innermost first, then outermost).
def double_ensure
  begin
    return 10
  ensure
    puts "10: inner-ensure"
    begin
      raise "c"
    rescue
    end
  end
ensure
  puts "10: outer-ensure"
  begin
    raise "d"
  rescue
  end
end
p double_ensure

# 11. Iterator-block break through a begin/ensure inside the block,
#     with a contained raise.
r11 = [10, 20].each do |x|
  begin
    break x + 1
  ensure
    begin
      raise "z"
    rescue
    end
  end
end
p r11
