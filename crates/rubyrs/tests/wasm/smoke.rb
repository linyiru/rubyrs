# Wasm smoke test — runs the built `rubyrs.wasm` (wasm32-wasip1,
# `cext` off) under wasmtime and asserts byte-for-byte stdout
# against `smoke.expected` (which is generated from CRuby).
#
# Covers a deliberate spread of subset features to catch
# regressions specific to the wasi build shape:
#
#   - puts / string interpolation
#   - integer literals + arithmetic + comparison
#   - Range#each + block + accumulator
#   - Array literal + `.map { ... }` + `.sum`
#   - Hash literal + iteration
#   - begin/rescue + raise (uses RuntimeError)
#   - while loop + break
#   - method def + def self.x (singleton)
#
# Intentionally small — the wasm CI lane runs this with a fresh
# wasmtime install per build; bloating it slows every PR. The
# diff_cruby suite is the larger correctness net (host-only).

puts "hello from wasm"

puts (1..5).map { |i| i * i }.sum

squares = []
(1..3).each { |i| squares << i ** 2 }
puts squares.inspect

h = { "a" => 1, "b" => 2 }
h.each { |k, v| puts "#{k}=#{v}" }

begin
  raise "boom"
rescue => e
  puts "rescued: #{e.message}"
end

i = 0
v = while true
  i += 1
  break i * 10 if i == 4
end
puts "broke at #{i}, value #{v}"

class Counter
  def initialize(n)
    @n = n
  end
  def double
    @n * 2
  end
  def self.from_str(s)
    new(s.to_i)
  end
end

c = Counter.from_str("7")
puts c.double
