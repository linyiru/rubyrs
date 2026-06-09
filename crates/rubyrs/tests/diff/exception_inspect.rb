# Default Exception#inspect renders `#<ClassName: message>` (bare
# `#<ClassName>` when the message is empty). `p exc` must match
# `exc.inspect` — both now route through the same renderer, so the
# message is no longer dropped by `p`/`pp`.
# (Exceptions nested INSIDE a collection still render via Array/Hash
# #inspect's recursive to_inspect, a separate path — not covered here.)

# 1. builtin error via bound var and via $!
begin
  raise "boom"
rescue => e
  p e                      # #<RuntimeError: boom>
  p $!                     # #<RuntimeError: boom>
  puts e.inspect           # #<RuntimeError: boom>
  puts(e.inspect == "#<RuntimeError: boom>")  # true
end

# 2. specific builtin subclass
begin
  raise ArgumentError, "bad arg"
rescue => e
  p e                      # #<ArgumentError: bad arg>
end

# 3. user-defined subclass
class MyErr < StandardError; end
begin
  raise MyErr, "custom"
rescue => e
  p e                      # #<MyErr: custom>
end

# 4. empty message → bare #<ClassName>
begin
  raise RuntimeError, ""
rescue => e
  p e                      # #<RuntimeError>
end

# 5. default message (`.new` with no args → message == class name)
p RuntimeError.new        # #<RuntimeError: RuntimeError>
p MyErr.new               # #<MyErr: MyErr>
p ArgumentError.new("x")  # #<ArgumentError: x>

# 6. pp matches p
begin
  raise "via pp"
rescue => e
  pp e                     # #<RuntimeError: via pp>
end
