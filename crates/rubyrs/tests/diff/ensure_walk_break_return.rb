# `break`/`next` executing INSIDE an ensure body while a method-return
# walk (or block-break walk / exception unwind) is suspended in it —
# the b4/b4c divergence family, fixed by the WalkOrigin +
# SuspendCoord::loop_depth region model (see vm.rs WalkOrigin doc).
#
# ORACLE-STABILITY WARNING: this fixture must stay byte-identical
# across ALL CRuby 3.4.x patch versions AND both parsers — CI floats
# on "3.4"-latest while dev machines may pin older patches. CRuby
# 3.4.0/3.4.1's Prism compiler had a BUG WINDOW in exactly this
# corner ([Bug #21001], fixed by ruby/ruby 31905d9e "Allow escaping
# from ensures through next", backported in 3.4.2): a bogus end_label
# made `break`/`next` inside an ensure body crossed by a suspended
# walk behave differently from parse.y / 3.3.x / prism >= 3.4.2.
# Fourteen shapes that sat in that window were extracted to
# tests/embed/ensure_walk_divergences.rs as pinned goldens (B1, B2,
# B3, B5, C1, C2, D3, E2, E3, H1, I1, I2, K1, K4 — K4 HANGS forever
# on >= 3.4.2, which is what originally hung CI's oracle). Re-verify
# any new shape against at least 3.4.1 AND the newest 3.4.x (and
# ideally --parser=parse.y) before adding it here.
#
# The scale that remains (stable across every probed CRuby):
# contained loops inside the ensure region (A1-A3) resolve locally
# and the walk resumes; a walk from a non-local origin (block `each`
# B4c / Kernel#loop B4, block-break F1, exception unwind G1-G4,
# non-local return J1/J6/J8, in-block loop J2, lambda J3,
# define_method J7) that hits a `break` in a crossed ensure lands the
# break at the loop join and CANCELS the walk (an exception being
# unwound is swallowed). `next` supersedes a pending return in
# while-loops (D1/D2), and `next` in the ensure of a block's own
# value return leaves/overrides the value (K5/K6).

# A1. return pending; ensure contains `loop { break Y }` (contained)
def a1
  return :ret
ensure
  r = loop { break :brk }
  puts "A1 inner=#{r.inspect}"
end
puts "A1 => #{a1.inspect}"


# A2. return pending; ensure contains `[..].each { break Y }` (contained block break)
def a2
  return :ret
ensure
  r = [1, 2].each { break :brk }
  puts "A2 inner=#{r.inspect}"
end
puts "A2 => #{a2.inspect}"


# A3. return pending; ensure contains `while true; break Y; end` (contained)
def a3
  return :ret
ensure
  r = while true; break :brk; end
  puts "A3 inner=#{r.inspect}"
end
puts "A3 => #{a3.inspect}"

# ---- B. loop OUTSIDE the ensure region, block-iterator variants ----
# (The while/until variants B1-B3/B5 — and the whole nested-loop C,
# ensure-inside-ensure E, retry H and sequential-loop I families that
# ride the same machinery — live in the embed goldens: their CRuby
# output flipped at 3.4.2.)


# B4. `loop {}` iterator (block-based) outside; return pending; ensure breaks
def b4
  loop do
    begin
      return :ret
    ensure
      break :brk
    end
  end
  puts "B4 after-loop reached"
  :after
end
puts "B4 => #{b4.inspect}"


# B4c. `[..].each {}` iterator outside; return pending; ensure breaks
def b4c
  [1, 2].each do |x|
    begin
      return :ret
    ensure
      break :brk
    end
  end
  puts "B4c after-loop reached"
  :after
end
puts "B4c => #{b4c.inspect}"

# ---- D. next in while-loops (stable across versions) ----


# D1. next in ensure of pending return (loop outside) — next supersedes
def d1
  i = 0
  while i < 2
    i += 1
    begin
      return :ret
    ensure
      next
    end
  end
  puts "D1 i=#{i}"
  :fell_through
end
puts "D1 => #{d1.inspect}"


# D2. next WITH VALUE in ensure of pending return
def d2
  i = 0
  while i < 2
    i += 1
    begin
      return :ret
    ensure
      next :nextval
    end
  end
  puts "D2 i=#{i}"
  :fell_through
end
puts "D2 => #{d2.inspect}"

# ---- F. break in ensure during BLOCK-break walk (not method return) ----


# F1. block passed to a method does `break`; walk crosses the method's
#     ensure; that ensure breaks a loop INSIDE the method (outside the
#     ensure region).
def f1_runner
  while true
    begin
      yield
    ensure
      break :ens_brk
    end
  end
  puts "F1 runner after-loop"
  :runner_after
end
def f1
  r = f1_runner { break :blk_brk }
  puts "F1 call result r=#{r.inspect}"
  :f1_done
end
puts "F1 => #{f1.inspect}"


# F2. block break walking through ensure that contains a CONTAINED loop-break
def f2_runner
  yield
  :runner_after
ensure
  r = while true; break :contained; end
  puts "F2 contained r=#{r.inspect}"
end
def f2
  r = f2_runner { break :blk_brk }
  puts "F2 call result r=#{r.inspect}"
  :f2_done
end
puts "F2 => #{f2.inspect}"


# F3. iterator-block break (each) suspended; ensure inside the BLOCK breaks the each?
def f3
  r = [1, 2].each do |x|
    begin
      break :blk_brk
    ensure
      puts "F3 block ensure x=#{x}"
    end
  end
  puts "F3 r=#{r.inspect}"
  :f3_done
end
puts "F3 => #{f3.inspect}"

# ---- G. break in ensure during EXCEPTION unwind ----


# G1. raise; ensure breaks loop (loop outside ensure). Swallowed?
def g1
  while true
    begin
      raise "g1-boom"
    ensure
      break :brk
    end
  end
  puts "G1 after-loop reached"
  :after
end
begin
  puts "G1 => #{g1.inspect}"
rescue => e
  puts "G1 raised #{e.message}"
end


# G2. raise; ensure has CONTAINED loop-break; exception should still propagate
def g2
  begin
    raise "g2-boom"
  ensure
    r = loop { break :contained }
    puts "G2 contained r=#{r.inspect}"
  end
  :after
end
begin
  puts "G2 => #{g2.inspect}"
rescue => e
  puts "G2 raised #{e.message}"
end


# G3. raise; ensure breaks; outer rescue in same method below the loop
def g3
  begin
    while true
      begin
        raise "g3-boom"
      ensure
        break :brk
      end
    end
    puts "G3 after-loop"
    :after
  rescue => e
    puts "G3 inner rescue #{e.message}"
    :rescued
  end
end
puts "G3 => #{g3.inspect}"


# G4. block-break in ensure during exception unwind ([..].each outside)
def g4
  r = [1, 2].each do |x|
    begin
      raise "g4-boom"
    ensure
      break :brk
    end
  end
  puts "G4 r=#{r.inspect}"
  :after
end
begin
  puts "G4 => #{g4.inspect}"
rescue => e
  puts "G4 raised #{e.message}"
end

puts "MATRIX DONE"

# J1. multi-frame: non-local return from block crossing an intermediate
#     method whose while+ensure breaks. Where does the break land?
def j1_mid
  while true
    begin
      yield
    ensure
      break :brk
    end
  end
  puts "J1 mid after-loop"
  :mid_after
end
def j1
  r = j1_mid { return :ret }
  puts "J1 outer r=#{r.inspect}"
  :outer_after
end
puts "J1 => #{j1.inspect}"

# J2. while-loop inside an iterator BLOCK; return walk suspended in
#     ensure inside that while; break targets the in-block while.
def j2
  [1].each do
    while true
      begin
        return :ret
      ensure
        break :brk
      end
    end
    puts "J2 in-block after-loop"
  end
  puts "J2 after each"
  :after
end
puts "J2 => #{j2.inspect}"

# J3. lambda-local return; while+ensure-break inside the lambda.
l = lambda do
  while true
    begin
      return :ret
    ensure
      break :brk
    end
  end
  puts "J3 after-loop"
  :after
end
puts "J3 => #{l.call.inspect}"

# J5. break in ensure during a suspended LOOP-BREAK walk (break crossing
#     an ensure, and THAT ensure breaks a different (outer) lexical loop).
def j5
  r_outer = while true
    r_inner = while true
      begin
        break :inner_brk
      ensure
        break :outer_brk
      end
    end
    puts "J5 inner join r=#{r_inner.inspect}"
    break :outer_fell
  end
  puts "J5 outer join r=#{r_outer.inspect}"
  :after
end
puts "J5 => #{j5.inspect}"

# J6. non-local return (from block) whose TARGET frame holds the
#     while+ensure-break. Does the artifact apply at the return's own
#     target frame when the return is non-local?
def j6
  while true
    begin
      [1].each { return :ret }
    ensure
      break :brk
    end
  end
  puts "J6 after-loop"
  :after
end
puts "J6 => #{j6.inspect}"

# J7. define_method body (block ISeq at compile time, method at runtime):
#     return + while + ensure-break.
class J7C
  define_method(:m) do
    while true
      begin
        return :ret
      ensure
        break :brk
      end
    end
    puts "J7 after-loop"
    :after
  end
end
puts "J7 => #{J7C.new.m.inspect}"

# J8. proc (non-lambda) called from a method: return in proc is
#     non-local to the enclosing method; proc body has while+ensure-break.
def j8
  pr = proc do
    while true
      begin
        return :ret
      ensure
        break :brk
      end
    end
    puts "J8 proc after-loop"
    :proc_after
  end
  r = pr.call
  puts "J8 r=#{r.inspect}"
  :after
end
puts "J8 => #{j8.inspect}"

# K5. next in the ensure of the block's own terminal-value return
r = [1, 2].map do |x|
  begin
    :v
  ensure
    next
  end
end
puts "K5 r=#{r.inspect}"

# K6. next WITH VALUE in the ensure of the block's own value return
r = [1, 2].map do |x|
  begin
    :v
  ensure
    next :override
  end
end
puts "K6 r=#{r.inspect}"

# L1. bytecode yielder (no Kernel#loop): break in block ensure during
#     the return walk — the B4 hang shape minimized.
def l1_yielder
  while true
    yield
  end
end
def l1
  l1_yielder do
    begin
      return :ret
    ensure
      break :brk
    end
  end
  puts "L1 after"
  :after
end
p l1

# L2. single-yield variant (walk crosses, no loop to strand).
def l2_yielder
  yield
  :yielder_done
end
def l2
  l2_yielder do
    begin
      return :ret
    ensure
      break :brk
    end
  end
  puts "L2 after"
  :after
end
p l2

puts "ensure_walk_break_return done"
