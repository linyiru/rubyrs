# Exception#cause is set implicitly: raising while another exception is
# being handled chains the new one's #cause to it.
begin
  begin
    raise "inner"
  rescue
    raise "outer"
  end
rescue => e
  p e.message
  p e.cause&.message
end

# No cause when not raised inside a rescue.
begin
  raise "top"
rescue => e
  p e.cause.inspect
end

# A bare re-raise keeps cause nil (same object, not its own cause).
begin
  begin
    raise "x"
  rescue
    raise
  end
rescue => e
  p [e.message, e.cause.inspect]
end

# A 3-level chain.
begin
  begin
    begin
      raise "one"
    rescue
      raise "two"
    end
  rescue
    raise "three"
  end
rescue => e
  p [e.message, e.cause.message, e.cause.cause.message]
end

# A class raise inside a rescue also chains.
begin
  begin
    raise ArgumentError, "arg"
  rescue
    raise TypeError, "typ"
  end
rescue => e
  p [e.class, e.cause.class, e.cause.message]
end
