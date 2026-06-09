# Range#step with Float bounds/step (block + no-block Enumerator) and
# inclusive vs exclusive ranges; Int+Int+Int fast path unchanged.
p (1..10).step(3).to_a
p (1...10).step(3).to_a
p (1.0..2.0).step(0.5).to_a
p (1.0...2.0).step(0.5).to_a       # exclusive drops 2.0
p (1.0..3.0).step(0.5).to_a
p (1.0...3.0).step(0.5).to_a       # exclusive drops 3.0
p (0.0..1.0).step(0.3).to_a
p (1..10).step(0.5).to_a.length    # Int range, Float step
p (2.0..5.0).step(1).to_a          # Float range, Int step
r = []; (1.0..2.0).step(0.5) { |x| r << x }; p r
r = []; (0..10).step(2) { |x| r << x }; p r
p((1.0..3.0).step(0.5) { |x| break x if x > 2.0 })
begin; (1.0..2.0).step(0) { |x| }; rescue => e; p [e.class, e.message]; end
