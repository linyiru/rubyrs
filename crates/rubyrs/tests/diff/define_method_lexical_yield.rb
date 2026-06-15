# A define_method/define_singleton_method body is a Proc: `yield`
# inside it reaches the block of the method that ran define_method
# (lexical), NOT the block passed when the defined method is called.
# zeitwerk's test support does
# `define_singleton_method(:teardown) { yield; super() }`.

class C
  def register(&_unused)
    define_singleton_method(:run) do
      "[#{yield}]"           # yields to register's block
    end
  end
end

c = C.new
c.register { "lexical" }
p c.run                       # "[lexical]"
p c.run { "caller" }          # still "[lexical]" — caller's block is NOT yielded
c.register { "second" }
p c.run                       # "[second]" — re-registration rebinds

# Nested block inside the define_method body still reaches the lexical yield.
class E
  def setup
    define_singleton_method(:go) do
      [1, 2].map { yield }
    end
  end
end
e = E.new
e.setup { "y" }
p e.go                        # ["y", "y"]

# Argument forwarding alongside the lexical yield.
class F
  def build
    define_singleton_method(:fmt) do |x|
      "#{yield}:#{x}"
    end
  end
end
f = F.new
f.build { "tag" }
p f.fmt(7)                    # "tag:7"
