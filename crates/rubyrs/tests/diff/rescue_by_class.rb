# --- class hierarchy used throughout ---
class AppError < StandardError
end
class NotFound < AppError
end
class Permission < AppError
end
class Network < StandardError
end

# Catch by exact class
begin
  raise NotFound, "missing"
rescue NotFound => e
  puts "caught NotFound: #{e.message}"
end

# Catch by superclass — NotFound is an AppError
begin
  raise NotFound, "missing"
rescue AppError => e
  puts "caught as AppError: #{e.message}"
end

# Catch by StandardError catches everything in StandardError tree
begin
  raise Permission, "denied"
rescue StandardError => e
  puts "stderr caught: #{e.message}"
end

# Non-matching rescue lets the exception keep unwinding to a matching one
begin
  begin
    raise Network, "down"
  rescue NotFound => e
    puts "inner caught (should not run)"
  end
rescue Network => e
  puts "outer caught: #{e.message}"
end

# Multiple rescue clauses — first matching wins, in source order
def classify(exc)
  begin
    raise exc, "boom"
  rescue NotFound => e
    "nf:#{e.message}"
  rescue AppError => e
    "ae:#{e.message}"
  rescue Network => e
    "nw:#{e.message}"
  end
end

puts classify(NotFound)
puts classify(Permission)
puts classify(Network)

# Bare rescue still works (= StandardError filter)
begin
  raise "anonymous"
rescue => e
  puts "bare: #{e.message}"
end

# Bare rescue does NOT catch a NON-StandardError subclass.
# `Exception` itself, like ResourceExhausted under it, lives outside
# the StandardError subtree — but we don't `raise Exception` directly
# here because rubyrs's `raise ExceptionClass` requires the class to be
# instantiable through `new`, and Exception works in CRuby.
class CustomFatal < Exception
end

begin
  begin
    raise CustomFatal, "fatal"
  rescue => e
    puts "bare swallowed CustomFatal (should not run)"
  end
rescue Exception => e
  puts "explicit Exception caught: #{e.message}"
end

# Bind variable is optional — `rescue NotFound` without `=> e` still
# catches but doesn't bind.
begin
  raise NotFound, "no-bind"
rescue NotFound
  puts "caught without binding"
end

# NOTE: `rescue UndefinedClass` behaviour diverges from CRuby — see
# docs/SUBSET.md. rubyrs silently skips the clause (no match); CRuby
# raises NameError when the rescue would fire. We don't exercise that
# in the diff harness; the embed-level test pins our behaviour.

# Rescue + ensure together — ensure must always run, rescue must catch
result = nil
order = []
begin
  begin
    order << "body"
    raise NotFound, "and ensure"
  rescue NotFound => e
    order << "rescue"
    result = e.message
  ensure
    order << "ensure"
  end
end
puts result
puts order[0]
puts order[1]
puts order[2]
