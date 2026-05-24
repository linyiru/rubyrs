class MyError < StandardError
end

begin
  raise MyError.new("boom")
rescue => e
  puts e.message
  puts e.to_s
end

# Nested rescue: inner re-raises a custom exception, outer catches.
class FileNotFound < StandardError
end

def load(path)
  raise FileNotFound.new("missing: " + path)
end

begin
  load("config.yml")
rescue => e
  puts e.message
end

# Distinguish exception classes via direct .class check on the user side.
class Boom < StandardError; end
class Whoops < StandardError; end

begin
  raise Boom.new("first")
rescue => e1
  puts e1.message
end

begin
  raise Whoops.new("second")
rescue => e2
  puts e2.message
end
