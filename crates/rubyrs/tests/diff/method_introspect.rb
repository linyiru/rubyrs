# Method#arity and Method#parameters introspection. Same shape
# for Method and UnboundMethod.

class Calc
  def add(a, b); a + b; end                 # arity 2 / [[:req,:a],[:req,:b]]
  def greet(name, greeting = "hi"); "#{greeting}, #{name}"; end  # arity -2
  def collect(*xs); xs; end                 # arity -1 / [[:rest,:xs]]
  def configure(host:, port: 80); end       # arity 1 (req kwargs only)
  def absorb(**opts); opts; end             # arity -1 / [[:keyrest,:opts]]
end

c = Calc.new

m = c.method(:add)
puts m.arity                                # 2
puts m.parameters.inspect                   # [[:req, :a], [:req, :b]]

m = c.method(:greet)
puts m.arity                                # -2
puts m.parameters.inspect                   # [[:req, :name], [:opt, :greeting]]

m = c.method(:collect)
puts m.arity                                # -1
puts m.parameters.inspect                   # [[:rest, :xs]]

m = c.method(:configure)
puts m.arity                                # -2  (host: required, port: optional)
puts m.parameters.inspect                   # [[:keyreq, :host], [:key, :port]]

m = c.method(:absorb)
puts m.arity                                # -1
puts m.parameters.inspect                   # [[:keyrest, :opts]]

# UnboundMethod shares the same surface.
u = c.method(:add).unbind
puts u.arity                                # 2
puts u.parameters.inspect                   # [[:req, :a], [:req, :b]]
