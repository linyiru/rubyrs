# Primitive errors are now Ruby-level exceptions: scripts can
# rescue them just like CRuby. Each block here demonstrates a
# different primitive raise + a matching rescue.

# NoMethodError from a method-on-nil
# NOTE: the message format ("for nil" vs "for NilClass", backtick
# vs apostrophe) differs between rubyrs and CRuby. We assert
# class + structural shape, not exact bytes.
begin
  nil.foo
rescue NoMethodError => e
  puts "no-method class: #{e.class.name}"
  puts e.message.include?("foo")
end

# Bare `rescue` catches via StandardError chain
begin
  nil.bar
rescue => e
  puts "bare: #{e.class.name}"
end

# KeyError from Hash#fetch
begin
  {a: 1}.fetch(:missing)
rescue KeyError => e
  puts "key: #{e.message}"
end

# `rescue StandardError` catches KeyError (KeyError < IndexError < StandardError)
begin
  {}.fetch(:nope)
rescue StandardError => e
  puts "std: #{e.class.name}"
end

# ArgumentError from wrong arg count
def takes_two(a, b)
  a + b
end
begin
  takes_two(1)
rescue ArgumentError => e
  puts "arg: #{e.message}"
end

# RuntimeError from bare `raise "msg"`
begin
  raise "boom"
rescue RuntimeError => e
  puts "runtime: #{e.message}"
end

# Rescue chain — first matching class wins
begin
  nil.foo
rescue ArgumentError => e
  puts "wrong arg"
rescue NoMethodError => e
  puts "right: nomethod"
rescue StandardError => e
  puts "wrong std"
end

# Non-matching rescue falls through; outer rescue catches
begin
  begin
    raise "inner"
  rescue NoMethodError => e
    puts "inner should not match"
  end
rescue RuntimeError => e
  puts "outer caught: #{e.message}"
end

# Rescue inside a block — primitive error in iteration is caught
results = []
[1, 2, 3].each do |n|
  begin
    raise "bad #{n}" if n == 2
    results << n
  rescue RuntimeError => e
    results << "caught #{e.message}"
  end
end
puts results[0]
puts results[1]
puts results[2]

# Method that catches its own primitive errors
def safe_div(a, b)
  begin
    a / b
  rescue StandardError => e
    "error: #{e.class.name}"
  end
end
# We don't catch ZeroDivisionError (no Int#/ check), but
# NoMethodError on type mismatch:
puts safe_div(10, 2)
# Skip the divide-by-zero case — that's a separate gap.

# The `e.message` and `e.class` (and class.name) all work on
# the synthesised exception instances.
begin
  "x".to_sym + 5
rescue => e
  puts "type-y: #{e.class.name}"
end
