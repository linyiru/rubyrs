# Two-argument `raise Class, "msg"` form. Synthesises
# `Class.new("msg")`, runs the class's initialize (which sets
# @message), and raises the resulting instance.

# Basic two-arg.
begin
  raise ArgumentError, "bad input"
rescue ArgumentError => e
  puts e.class.name
  puts e.message
end

# StandardError ancestor catches it.
begin
  raise TypeError, "wrong type"
rescue StandardError => e
  puts e.class.name
  puts e.message
end

# Custom exception class.
class ConfigError < StandardError
end

begin
  raise ConfigError, "missing key"
rescue ConfigError => e
  puts e.class.name
  puts e.message
end

# Caught by parent class.
begin
  raise ConfigError, "ancestor catch"
rescue StandardError => e
  puts e.class.name
  puts e.message
end

# Re-raise with a different class.
begin
  begin
    raise ArgumentError, "low level"
  rescue ArgumentError
    raise RuntimeError, "wrapped"
  end
rescue RuntimeError => e
  puts e.class.name
  puts e.message
end

# Two-arg raise inside a method.
def validate(n)
  raise ArgumentError, "negative" if n < 0
  raise RangeError, "too big" if n > 100
  n
end

class RangeError < StandardError
end

puts validate(50)
begin
  validate(-1)
rescue ArgumentError => e
  puts e.message
end
begin
  validate(200)
rescue RangeError => e
  puts e.message
end

# Two-arg with a class hierarchy: most-specific handler wins.
class IOError < StandardError
end
class FileNotFound < IOError
end

begin
  raise FileNotFound, "no such file"
rescue IOError => e
  puts e.class.name
  puts e.message
end

# Comparable's <=> = nil → raises ArgumentError (the refinement).
class Sometimes
  include Comparable
  attr_reader :n
  def initialize(n)
    @n = n
  end
  def <=>(other)
    return nil if other.nil?
    return nil if other.class.name != "Sometimes"
    @n <=> other.n
  end
end

a = Sometimes.new(5)
b = Sometimes.new(3)
puts a > b
puts a >= b

# Incomparable pair → ArgumentError from Comparable's <.
begin
  a < nil
rescue ArgumentError => e
  puts "rescued <: #{e.class.name}"
end

# But == still returns false rather than raising.
puts a == nil
puts a == "string"
