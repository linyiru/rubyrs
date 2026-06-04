# Exception hierarchy parity — newly-added classes
# (EncodingError + Encoding::*, Math::DomainError,
# SystemCallError + Errno::*, SecurityError, NoMemoryError)
# resolve at parse time, have CRuby-aligned ancestors, and
# slot into the rescue walker correctly.

# Hierarchy renders (first 3 entries — Object-tail divergence
# documented as Tier-1).
puts EncodingError.ancestors.first(3).inspect
puts Encoding::CompatibilityError.ancestors.first(4).inspect
puts Encoding::ConverterNotFoundError.ancestors.first(4).inspect
puts Encoding::InvalidByteSequenceError.ancestors.first(4).inspect
puts Encoding::UndefinedConversionError.ancestors.first(4).inspect
puts Math::DomainError.ancestors.first(3).inspect
puts SystemCallError.ancestors.first(3).inspect
puts Errno::ENOENT.ancestors.first(3).inspect
puts Errno::EACCES.ancestors.first(3).inspect
puts Errno::EEXIST.ancestors.first(3).inspect
puts Errno::ENOTDIR.ancestors.first(3).inspect
puts Errno::EISDIR.ancestors.first(3).inspect
puts Errno::EINVAL.ancestors.first(3).inspect
puts Errno::ENOSPC.ancestors.first(3).inspect
puts Errno::EPIPE.ancestors.first(3).inspect
puts Errno::ECONNREFUSED.ancestors.first(3).inspect
puts Errno::ECONNRESET.ancestors.first(3).inspect
puts SecurityError.ancestors.first(2).inspect
puts NoMemoryError.ancestors.first(2).inspect

# Rescue via parent class — Encoding::* catches via
# EncodingError.
begin
  raise Encoding::UndefinedConversionError, "bad enc"
rescue EncodingError => e
  puts "encoding parent caught: #{e.class}"
end

# Errno::* catches via SystemCallError AND via StandardError.
begin
  raise Errno::ENOENT, "no such file"
rescue SystemCallError => e
  puts "errno via SystemCallError: #{e.class}"
end

begin
  raise Errno::EACCES, "denied"
rescue StandardError => e
  puts "errno via StandardError: #{e.class}"
end

# Math::DomainError caught by StandardError (under it).
begin
  raise Math::DomainError, "out of domain"
rescue StandardError => e
  puts "math via StandardError: #{e.class}"
end

# SecurityError / NoMemoryError sit `< Exception`, NOT
# `< StandardError`. Bare `rescue` (StandardError filter)
# must NOT swallow them — same security-posture rationale
# as ResourceExhausted / SystemStackError / SignalException.
caught_at_bare = nil
caught_at_outer = nil
begin
  begin
    raise SecurityError, "policy"
  rescue => e
    caught_at_bare = e.class
  end
rescue SecurityError => e
  caught_at_outer = e.class
end
puts "security bare_rescue=#{caught_at_bare.inspect}"
puts "security outer=#{caught_at_outer}"

caught_at_bare = nil
caught_at_outer = nil
begin
  begin
    raise NoMemoryError, "oom"
  rescue => e
    caught_at_bare = e.class
  end
rescue NoMemoryError => e
  caught_at_outer = e.class
end
puts "no_mem bare_rescue=#{caught_at_bare.inspect}"
puts "no_mem outer=#{caught_at_outer}"

# `rescue Exception` is the broader catch-all that DOES include
# the outside-StandardError classes.
begin
  raise SecurityError, "x"
rescue Exception => e
  puts "rescue_Exception=#{e.class}"
end

# `is_a?` walks the hierarchy correctly.
e = Errno::ENOENT.new("test")
puts "isa SystemCallError=#{e.is_a?(SystemCallError)}"
puts "isa StandardError=#{e.is_a?(StandardError)}"
puts "isa Exception=#{e.is_a?(Exception)}"
puts "isa Errno::EACCES=#{e.is_a?(Errno::EACCES)}"
