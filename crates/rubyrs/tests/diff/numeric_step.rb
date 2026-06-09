# Numeric#step — positional (limit[, by]) and keyword (to:/by:) forms,
# Integer and Float progressions; no-block returns an Enumerator.
p 1.step(10, 3).to_a
p 10.step(1, -2).to_a
p 1.step(10).to_a
p 5.step(1).to_a                # wrong direction → empty
p 1.step(by: 2, to: 9).to_a
p 1.step(to: 5).to_a
p 1.step(2, 0.5).to_a           # int recv + float step → floats
p 0.step(1, 0.3).to_a
p 1.0.step(3.0, 0.5).to_a
p 1.step(1).to_a
p 1.step(0, -1).to_a
p 10.step(1, 2).to_a
p 2.5.step(5).to_a
p 1.step(10, 3).map { |x| x * x }
r = []; 1.step(10, 2) { |x| r << x }; p r
p(1.step(3) { |x| })           # block form returns the receiver
p(1.step(5, 2) { |x| break x * 10 if x == 3 })
begin; 1.step(10, 0) { |x| }; rescue => e; p [e.class, e.message]; end
p 1.respond_to?(:step)
p 1.5.respond_to?(:step)
