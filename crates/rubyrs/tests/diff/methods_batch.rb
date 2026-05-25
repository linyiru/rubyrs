# Array#each_with_object — thread an unchanging accumulator
result = [1, 2, 3, 4].each_with_object([]) { |x, memo| memo << x * 10 }
puts result.inspect
puts result.length

# Memo object identity is preserved — same instance throughout
seed = {count: 0}
result = [1, 2, 3].each_with_object(seed) { |_x, m| m[:count] = m[:count] + 1 }
puts result[:count]
puts result.equal?(seed)

# Array#partition — splits into [matching, non-matching]
r = [1, 2, 3, 4, 5, 6].partition { |n| n.even? }
puts r[0].inspect
puts r[1].inspect

# Empty array — two empty arrays
r2 = [].partition { |_x| true }
puts r2[0].inspect
puts r2[1].inspect

# All-match / no-match
r3 = [2, 4, 6].partition { |n| n.even? }
puts r3[0].length
puts r3[1].length
r4 = [1, 3, 5].partition { |n| n.even? }
puts r4[0].length
puts r4[1].length

# Hash#each_with_index — yields ([k, v], idx)
{a: 1, b: 2, c: 3}.each_with_index { |pair, i| puts "#{i}: #{pair[0]}=#{pair[1]}" }

# Hash#map — yields (k, v), returns Array of block results
nums = {one: 1, two: 2, three: 3}
puts nums.map { |k, v| "#{k}=#{v}" }.inspect
puts nums.map { |_k, v| v * 100 }.inspect
puts nums.collect { |k, _v| k.to_s.upcase }.inspect    # collect is an alias

# Empty hash — empty Array
puts({}.map { |k, v| k }.inspect)

# Hash#fetch — 3 forms
h = {a: 1, b: 2}
puts h.fetch(:a)
puts h.fetch(:b)

# 2-arg form with default
puts h.fetch(:missing, "default-string")
puts h.fetch(:missing, 0)
puts h.fetch(:missing, nil).nil?

# Block form — called with the missing key
puts h.fetch(:nope) { |k| "no #{k}" }
puts h.fetch(:zzz) { 999 }

# Existing key wins over default / block
puts h.fetch(:a, "ignored")
puts h.fetch(:a) { fail "should not be called" }

# Realistic idiom: config DSL with safe defaults
def build_config(opts)
  {
    host: opts.fetch(:host, "localhost"),
    port: opts.fetch(:port, 8080),
    tls:  opts.fetch(:tls,  false),
  }
end
cfg = build_config({host: "example.com", port: 443})
puts cfg[:host]
puts cfg[:port]
puts cfg[:tls]

cfg2 = build_config({})
puts cfg2[:host]
puts cfg2[:port]
puts cfg2[:tls]

# Composed: partition + each_with_object to count
xs = [1, 2, 3, 4, 5, 6, 7, 8]
stats = xs.each_with_object({pos: 0, even: 0}) do |n, acc|
  acc[:pos] = acc[:pos] + 1 if n > 0
  acc[:even] = acc[:even] + 1 if n.even?
end
puts stats[:pos]
puts stats[:even]
