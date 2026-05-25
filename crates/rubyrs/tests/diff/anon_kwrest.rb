# Anonymous **rest-keyword `def f(**)` — accepts arbitrary kwargs
# without binding them to a name. Common in forwarding shims and
# stub methods. CRuby semantics: leftover kwargs are silently
# absorbed, body has no way to read them (the lack of a name is
# the point).

def stub(**)
  "noop"
end
puts stub
puts stub(a: 1)
puts stub(a: 1, b: 2, c: 3)
puts "---"

# Mixed: positional + named kwarg + anonymous **rest.
def configure(target, name:, **)
  "#{target}:#{name}"
end
puts configure("db", name: "main", host: "x", port: 1)
puts configure("api", name: "v2")
puts "---"

# Inside a class — common shim pattern.
class Config
  def initialize(**)
    @ready = true
  end
  def ready?
    @ready
  end
end
c = Config.new(host: "localhost", port: 5432, user: "admin")
puts c.ready?
