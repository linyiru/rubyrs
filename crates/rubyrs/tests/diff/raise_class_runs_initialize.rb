# `raise SomeExceptionClass` (a class, no explicit `.new`) must run the
# class's `initialize` — it's `SomeClass.exception` == `SomeClass.new`.
# Previously rubyrs stamped @message = class name and skipped it.

class E < StandardError
  def initialize(m = "default")
    super
  end
end
begin; raise E; rescue => e; p e.message; end
begin; raise E, "given"; rescue => e; p e.message; end
p E.new.message

class F < StandardError
  def initialize
    super("fixed message")
  end
end
begin; raise F; rescue => e; p e.message; end

# Custom initialize that sets extra state.
class G < StandardError
  attr_reader :code
  def initialize(msg = "g error")
    super(msg)
    @code = 42
  end
end
begin
  raise G
rescue => e
  p [e.message, e.code]
end

# Plain class (default initialize) still gets the class name.
begin; raise ArgumentError; rescue => e; p e.message; end
begin; raise RuntimeError; rescue => e; p e.message; end

# raise with an instance is unchanged.
begin; raise E.new("inst"); rescue => e; p e.message; end
