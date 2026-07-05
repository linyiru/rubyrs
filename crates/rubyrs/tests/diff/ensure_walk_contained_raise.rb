# A `return` / `break` / `next` / block-`break` walking an `ensure`
# chain suspends inside the ensure body while it runs. An exception
# raised AND rescued entirely WITHIN that body (directly, in a
# callee, via a host-raised error, via throw/catch, or via retry)
# must NOT cancel the suspended walk — CRuby resumes it at the
# body's end. Regression fixture for "ICE: EndEnsure with empty
# stack on exception path" (net/http in sandboxed environments):
# the unwinder used to clear the pending transfer eagerly at every
# raise, so the walk's EndEnsure fell into the exception re-raise
# path with nothing on the operand stack.

# 1. return-through-ensure + raise rescued inline in the body.
def contained_inline
  return 1
ensure
  begin
    raise "boom"
  rescue
    puts "1: rescued"
  end
end
p contained_inline

# 2. Same, but the raise+rescue live in a CALLEE of the ensure body
#    (the unwind never even reaches the suspended frame).
def cleanup_rescues
  begin
    raise "boom"
  rescue
    :ok
  end
end
def contained_callee
  return 2
ensure
  puts "2: cleanup=#{cleanup_rescues.inspect}"
end
p contained_callee

# 3. HOST-raised error (Integer() raises from native code, not a
#    Ruby `raise`) rescued inside the ensure body — the net/http
#    shape: socket-layer errors surfacing mid-cleanup.
def contained_host_raise
  return 3
ensure
  begin
    Integer("zzz")
  rescue ArgumentError => e
    puts "3: host-rescued #{e.class}"
  end
end
p contained_host_raise

# 4. while/break-through-ensure + contained raise.
r4 = while true
  begin
    break 4
  ensure
    begin
      raise "boom"
    rescue
      puts "4: rescued"
    end
  end
end
p r4

# 5. next-through-ensure + contained raise.
i5 = 0
while i5 < 3
  i5 += 1
  begin
    next
  ensure
    begin
      raise "boom"
    rescue
    end
  end
end
puts "5: i=#{i5}"

# 6. Block-break through the yielding method's ensure + contained
#    raise.
def yielding6
  yield
ensure
  begin
    raise "boom"
  rescue
    puts "6: rescued"
  end
end
p(yielding6 { break 6 })

# 7. throw/catch fully contained in the ensure body (throw rides
#    the exception machinery internally — it must not cancel the
#    suspended return either).
def contained_throw
  return 7
ensure
  puts "7: caught=#{catch(:t) { throw :t, :sig }.inspect}"
end
p contained_throw

# 8. retry inside the ensure body's begin/rescue — several
#    contained unwinds in a row before the walk resumes.
def contained_retry
  return 8
ensure
  t = 0
  begin
    t += 1
    raise "again" if t < 3
  rescue
    retry
  end
  puts "8: tries=#{t}"
end
p contained_retry

# 9. Nested method-break: the suspended ensure body CALLS a method
#    whose block does a non-local return — a second, inner walk
#    that must complete without clobbering the outer one.
def helper9
  [1].each { return :from_helper }
end
def contained_nested_return
  return 9
ensure
  puts "9: helper=#{helper9.inspect}"
end
p contained_nested_return

# 10. Nested ensure inside the suspended body, its own begin/rescue
#     contained one level deeper.
def contained_nested_ensure
  return 10
ensure
  begin
    begin
      raise "inner"
    rescue
    end
  ensure
    puts "10: inner-ensure"
  end
end
p contained_nested_ensure

# 11. Exception-path ensure (raise unwinding out) whose body has a
#     nested suspended break-walk with a contained raise — the
#     original exception must still re-raise at the body's tail.
def exc_path_nested
  raise "outer"
ensure
  while true
    begin
      break
    ensure
      begin
        raise "inner"
      rescue
        puts "11: inner-rescued"
      end
    end
  end
end
begin
  exc_path_nested
rescue => e
  puts "11: caught #{e.message}"
end

# 12. return of a HEAP value through an allocating ensure body —
#     the pending value must survive GC while the body runs
#     (STRESS_GC-sensitive).
def heap_value_return
  return [1, 2, 3]
ensure
  20.times { |n| "alloc-#{n}" * 4 }
  begin
    raise "boom"
  rescue
  end
end
p heap_value_return

# 13. Both walks parked at IDENTICAL coordinates (a while/break
#     ensure as the first statement of an outer suspended ensure
#     body) — innermost must resume first, then the outer return.
def identical_coords
  return 13
ensure
  while true
    begin
      break
    ensure
      puts "13: loop-ensure"
    end
  end
end
p identical_coords
