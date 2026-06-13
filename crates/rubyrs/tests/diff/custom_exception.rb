class MyError < StandardError
end

begin
  raise MyError.new("boom")
rescue => e
  puts e.message
  puts e.to_s
end

# 3-arg raise(class, message, backtrace): the exception is built via
# #exception with the MESSAGE ONLY (not the backtrace as a 2nd ctor
# arg), so a class whose initialize takes 0..1 doesn't ArgumentError.
# rack's QueryParser does `raise InvalidParameterError, e.message,
# e.backtrace`. The backtrace arg is dropped.
class E3 < StandardError; end
begin; raise E3, "boom3", ["a:1", "b:2"]; rescue => x; p [x.class, x.message]; end
begin; raise E3.new("orig"), "override", []; rescue => x; p [x.class, x.message]; end
class E1msg < StandardError
  def initialize(m = "default"); super; end
end
begin; raise E1msg, "msg1", caller; rescue => x; p [x.class, x.message]; end

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
