# Int / 0 — script-catchable ZeroDivisionError
begin
  1 / 0
rescue ZeroDivisionError => e
  puts "div: #{e.message}"
end

# Int % 0 — same trap
begin
  10 % 0
rescue ZeroDivisionError => e
  puts "mod: #{e.message}"
end

# StandardError catches ZeroDivisionError via the class chain
begin
  100 / 0
rescue => e
  puts "bare: #{e.class.name}"
end

# Inside a method, with a runtime-computed divisor
def safe_divide(a, b)
  begin
    a / b
  rescue ZeroDivisionError
    nil
  end
end

puts safe_divide(10, 2)
puts safe_divide(10, 0).nil?
puts safe_divide(7, 3)

# Float / 0.0 — NOT an error in CRuby (IEEE 754)
puts (1.0 / 0.0)
puts (-1.0 / 0.0)
puts (0.0 / 0.0).nan?

# Mixed Int / Float where Float is 0.0 — Float coercion wins,
# result is Infinity (no exception)
puts (5 / 0.0)
puts (-5 / 0.0)

# Inside a block — caught by the block's surrounding begin
results = []
[1, 2, 0, 4].each do |d|
  begin
    results << 100 / d
  rescue ZeroDivisionError
    results << "skipped"
  end
end
puts results[0]
puts results[1]
puts results[2]
puts results[3]

# Array#inject(:/) with a 0 element — raises and can be rescued
begin
  [10, 2, 0, 5].inject(:/)
rescue ZeroDivisionError => e
  puts "inject: #{e.message}"
end
