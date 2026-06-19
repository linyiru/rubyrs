# A `throw` to a live tag is a non-local jump: no `rescue` between
# the throw and its `catch` may intercept it — not even a bare
# `rescue Exception` — but intervening `ensure` blocks still run.

# rescue Exception is transparent to the throw
r1 = catch(:halt) do
  begin
    throw :halt, 42
  rescue Exception => e
    "WRONGLY caught: #{e.class}"
  end
end
p r1

# ensure runs on the way out, rescue body does not
log = []
r2 = catch(:halt) do
  begin
    throw :halt, 99
  rescue Exception
    log << "caught"
    "wrong"
  ensure
    log << "ensure ran"
  end
end
p r2
p log

# nested rescues are all transparent
r3 = catch(:done) do
  begin
    begin
      throw :done, :ok
    rescue StandardError
      :inner
    end
  rescue Exception
    :outer
  end
end
p r3

# wrong-tag throw still surfaces as a real (rescuable) error
begin
  catch(:a) { throw :b, 1 }
rescue ArgumentError => e
  p "uncaught: #{e.message}"
end

# a real exception between is still caught normally
r4 = begin
  raise "boom"
rescue => e
  "ok: #{e.message}"
end
p r4
