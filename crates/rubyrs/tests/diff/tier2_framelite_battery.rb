# Tier-2 wave-4 FRAME-LITE battery (ADR 0037 wave 4).
#
# Frame-lite bodies run with NO interpreter frame: recv+args stay on the
# operand stack, locals live in a native spill slot, and any op the lite
# mode can't finish MATERIALIZES the real frame (the deferred push) and
# hands the rest of the body to the interpreter. This battery pins the
# acid contract: raises reached THROUGH a lite activation (materialize →
# interpreted raise) must produce the interpreter's exact backtrace and
# unwind behaviour; the materialize ownership transfer (non-trivial args)
# must be leak/double-free-clean (STRESS_GC covers that axis); and the
# decline/breaker paths must be value-invisible.
#
# Backtrace lines are normalized to "file:line" — CRuby 3.4 prints
# "in 'Leaf#add'" where rubyrs prints "in 'add'" (same normalization as
# tier2_writeback_battery / tier2_call_family).

class Leaf
  def initialize(v)
    @v = v
  end

  # Plain lite getter-ish predicate (CaseEqLit + BinOp shapes).
  def send_type?
    @v == :send
  end

  # 1-arg lite arithmetic body — the TypeError probe (Int guard fails on a
  # String arg → materialize → interpreted raise).
  def add(x)
    @v + x
  end

  # Lite StoreIvar — the FrozenError probe (frozen recv declines the lean
  # store → materialize → interpreted raise, canonical message + line).
  def set(v)
    @v = v
  end

  def val
    @v
  end

  # Branchy body whose Return merges from both arms (exercises the
  # real-stack return path).
  def pick(x)
    if x > 10
      :big
    else
      :small
    end
  end

  # `x.nil?` fusion on an ivar receiver.
  def missing?
    @v.nil?
  end

  # Non-trivial (Str) arg read → LoadLocal guard declines → materialize
  # with the arg's ownership TRANSFERRED from the stack slot to the frame.
  def keep(s)
    @tag = 1
    s
  end

  # Str-valued ivar read → lite ivar-get declines → materialize (chronic:
  # the bail-streak breaker eventually settles this proto to the framed
  # tier; values must be identical throughout).
  def label
    @label
  end

  def label=(s)
    @label = s
  end
end

leaves = (1..60).map { |i| Leaf.new(i) }

# Warm every shape well past any compile threshold (and past the breaker
# streak for the chronic-decline shapes).
acc = 0
leaves.each do |l|
  400.times do
    acc += l.add(2)
    acc += 1 if l.pick(l.val) == :big
    l.set(l.val)
    acc += 1 if l.send_type?
    acc += 1 if l.missing?
  end
end
puts acc

# 1. TypeError raised from inside a frame-lite activation at depth 1..3 —
#    backtrace lines identical to the interpreter's.
l = Leaf.new(5)
200.times { l.add(1) }

def depth1(l)
  l.add("boom")
end

def depth2(l)
  depth1(l)
end

def depth3(l)
  depth2(l)
end

begin
  depth3(l)
rescue TypeError => e
  puts "E1 #{e.class}"
  # CRuby lists the core `Integer#+` frame on the same line as the method
  # frame; rubyrs doesn't model core-method frames — dedupe consecutive
  # identical file:line entries so both engines print the user frames.
  lines = e.backtrace.first(5).map { |ln| ln[%r{[^/]+:\d+}] }
  lines.chunk_while { |a, b| a == b }.map(&:first).first(3).each { |ln| puts "  #{ln}" }
end

# 2. FrozenError from the lite StoreIvar decline → interpreted raise.
frozen = Leaf.new(1)
200.times { frozen.set(3) }
frozen.freeze
begin
  frozen.set(42)
rescue FrozenError => e
  puts "E2 #{e.class} #{e.backtrace.first[%r{[^/]+:\d+}]}"
end
puts frozen.val

# 3. ensure in CALLER frames running on unwind through a lite activation.
def with_ensure(l)
  marker = :before
  l.add(nil)
  marker = :after
ensure
  puts "ensure marker=#{marker}"
end

begin
  with_ensure(l)
rescue TypeError => e
  puts "E3 #{e.class}"
end

# 4. bare `raise` re-raise through a rescue around a lite activation.
begin
  begin
    l.add({})
  rescue TypeError
    raise
  end
rescue TypeError => e
  puts "E4 #{e.message}"
end

# 5. Non-trivial arg ownership transfer at materialize: the returned Str is
#    the SAME object the caller passed, intact after the transfer.
s = +"hello world"
got = nil
200.times { got = l.keep(s) }
puts got
puts got.equal?(s)

# 6. Str-valued ivar (chronic ivar-get decline → breaker settles): values
#    stay correct across the kill boundary.
l.label = "tagged"
out = []
100.times { out << l.label }
puts out.uniq.inspect

# 7. Float args on an Int-guarded body: guard declines → materialize →
#    interpreted arithmetic, correct values throughout (breaker may settle).
fl = Leaf.new(2)
200.times { fl.add(1) }
fsum = 0.0
50.times { fsum += fl.add(0.5) }
puts fsum

# 8. pick()'s branch merge + Symbol returns across many calls.
puts leaves.map { |x| x.pick(x.val) }.tally.sort.inspect

# 9. nil?-fusion answers.
puts [Leaf.new(nil).missing?, Leaf.new(0).missing?].inspect
