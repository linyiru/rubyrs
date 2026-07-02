# Closure capture is a SHARED BINDING: every closure capturing a local
# (and the defining scope itself) reads/writes the same slot, for the
# lifetime of any capturing closure — including after intermediate
# block frames pop. Pins the outer-chain routing model (Frame::own_start
# + Frame::outer_chain): before it, a block created INSIDE another block
# rebound outer locals on a dead per-invocation copy (Thread bodies,
# stored procs, Fibers all lost their writes), and escaped procs read
# stale snapshots.

puts "== A: def-local captured at block depth 1/2/3 =="

def a1; x = 0; 2.times { x += 1 }; x; end
puts "A1 def depth1 rebind: #{a1}"

def a2; x = 0; 2.times { 2.times { x += 1 } }; x; end
puts "A2 def depth2 rebind: #{a2}"

def a3; x = 0; 2.times { 2.times { 2.times { x += 1 } } }; x; end
puts "A3 def depth3 rebind: #{a3}"

def a2r; x = 7; seen = nil; 1.times { 1.times { seen = x } }; [seen, x].inspect; end
puts "A2r def depth2 read: #{a2r}"

def a2w; x = 0; 1.times { 1.times { x = 42 } }; x; end
puts "A2w def depth2 plain-assign: #{a2w}"

puts "== B: toplevel local at depth 1/2/3 =="

b1 = 0; 2.times { b1 += 1 }
puts "B1 toplevel depth1 rebind: #{b1}"

b2 = 0; 2.times { 2.times { b2 += 1 } }
puts "B2 toplevel depth2 rebind: #{b2}"

b3 = 0; 2.times { 2.times { 2.times { b3 += 1 } } }
puts "B3 toplevel depth3 rebind: #{b3}"

puts "== C: outer-block PARAMETER rebound from inner block =="

cres = []
[10, 20].each { |p| 2.times { p += 1 }; cres << p }
puts "C1 blockparam rebound-inside: #{cres.inspect}"

puts "== D: closure outliving the creating frame =="

def d1
  x = 0
  ps = []
  2.times { |i| ps << proc { x += i + 1 } }
  ps.each(&:call)
  ps.each(&:call)
  x
end
puts "D1 proc created depth1, called after: #{d1}"

def d2
  x = 0
  ps = []
  1.times { 2.times { |i| ps << proc { x += i + 1 } } }
  ps.each(&:call)
  ps.each(&:call)
  x
end
puts "D2 proc created depth2, called after: #{d2}"

def d3
  ps = []
  1.times do
    y = 0
    2.times { ps << proc { y += 1 } }
    ps << proc { y }
  end
  # y's frame (the outer block invocation) has popped; the procs must
  # still share ONE y binding.
  ps[0].call; ps[1].call
  ps[2].call
end
puts "D3 block-local outlives block frame: #{d3}"

def d4
  x = 100
  p1 = nil
  1.times { p1 = proc { x += 1 } }
  p1.call
  x
end
puts "D4 proc made at depth1 called at depth0: #{d4}"

puts "== F: lambda / proc / Thread / Fiber bodies =="

def f1; x = 0; l = lambda { x += 1 }; 2.times { l.call }; x; end
puts "F1 lambda depth0-made: #{f1}"

def f2; x = 0; 1.times { l = lambda { x += 1 }; l.call; l.call }; x; end
puts "F2 lambda made at depth1: #{f2}"

x_t = 0
ts = []
2.times { ts << Thread.new { x_t += 1 } }
ts.each(&:join)
puts "F3 Thread.new inside times, toplevel local: #{x_t}"

def f4; x = 0; ts = []; 2.times { ts << Thread.new { x += 1 } }; ts.each(&:join); x; end
puts "F4 Thread.new inside times, def local: #{f4}"

def f5
  x = 0
  fs = []
  2.times { fs << Fiber.new { x += 1 } }
  fs.each(&:resume)
  x
end
puts "F5 Fiber.new inside times, def local: #{f5}"

l3 = 0
t3 = []
2.times { |i| t3 << Thread.new { l3 += (i + 1) } }
t3.each(&:join)
puts "F6 threads add outer-block param: #{l3}"

puts "== G: control cases (no new scope) =="

def g1; x = 0; 1.times { i = 0; while i < 3; x += 1; i += 1; end }; x; end
puts "G1 while inside block rebinds outer: #{g1}"

def g2
  x = 0
  1.times { for i in 1..3 do x += i end }
  x
end
puts "G2 for-loop inside block: #{g2}"

puts "== H: sibling closures share the binding =="

def h1
  x = 0
  log = []
  1.times { 1.times { x += 1; log << x }; 1.times { x += 10; log << x } }
  log << x
  log.inspect
end
puts "H1 siblings at depth2: #{h1}"

def h2
  x = 0
  a = proc { x += 1 }
  b = proc { x += 10 }
  1.times { a.call; b.call }
  x
end
puts "H2 sibling procs: #{h2}"

puts "== I: instance_eval / class_eval =="

def i1
  x = 0
  o = Object.new
  o.instance_eval { x += 5 }
  x
end
puts "I1 instance_eval rebinds def local: #{i1}"

def i2
  x = 0
  1.times { Object.new.instance_eval { x += 5 } }
  x
end
puts "I2 instance_eval at depth2: #{i2}"

def i3
  x = 0
  String.class_eval { x += 3 }
  x
end
puts "I3 class_eval rebinds def local: #{i3}"

puts "== J: interleaved depth1/depth2 writes =="

def j1
  x = 0
  seen = []
  1.times do
    x += 1
    1.times { x += 1; seen << x }
    seen << x
  end
  seen << x
  seen.inspect
end
puts "J1 interleaved: #{j1}"

puts "== K: read-through, staleness, binder edges =="

def k1
  # The reading block is COPY-PATH (it contains an inner block), so the
  # read of x after the sibling's share-direct write must route to the
  # method cell, not this frame's snapshot.
  x = 0
  bump = proc { x += 9 }
  seen = nil
  1.times { probe = proc { }; bump.call; seen = x; probe.call }
  [seen, x].inspect
end
puts "K1 sibling write then read inside copy-path block: #{k1}"

def k2
  x = 0
  p1 = nil
  1.times { p1 = proc { x } }
  x = 42
  p1.call
end
puts "K2 escaped proc reads method write: #{k2}"

def k3
  a = 0; b = 0
  1.times { 1.times { a, b = 1, 2 } }
  [a, b].inspect
end
puts "K3 massign outer from depth2: #{k3}"

def k4
  x = 0
  add = nil
  1.times { 1.times { add = proc { |n| x += n } } }
  add.call(5)
  add.call(7)
  x
end
puts "K4 escaped depth2 proc with arg: #{k4}"

def k5
  e_seen = nil
  1.times do
    begin
      raise "boom"
    rescue => e
      e_seen = e.message
    end
  end
  e_seen
end
puts "K5 rescue bind inside block: #{k5}"

def k6
  # Body-introduced block locals stay per-invocation.
  probe = []
  m = proc { |v| t ||= v; probe << t }
  m.call(1)
  m.call(2)
  probe.inspect
end
puts "K6 proc body-local fresh per call: #{k6}"

puts "== L: fiber suspension visibility =="

l1 = 0
f = Fiber.new { l1 += 1; Fiber.yield; l1 += 1 }
f.resume
mid = l1
f.resume
puts "L1 fiber mid-suspend visibility: #{[mid, l1].inspect}"

def l2
  x = 0
  f = nil
  1.times { f = Fiber.new { x += 1; Fiber.yield; x += 10 } }
  f.resume
  a = x
  f.resume
  [a, x].inspect
end
puts "L2 fiber created at depth1: #{l2}"
