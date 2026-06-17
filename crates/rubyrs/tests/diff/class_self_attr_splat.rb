# `class << self; attr_reader(*NAMES); end` — attr_* with a runtime
# SPLAT of names (an Array constant) inside the singleton-class body.
# The names aren't known at compile time, so this desugars to a runtime
# `self.singleton_class.send(:attr_reader, *NAMES)`, defining class-level
# accessors. Surfaced by mail's multibyte/unicode.rb.
class Config
  ATTRS = [:host, :port]
  class << self
    attr_reader(*ATTRS)
    attr_writer(*ATTRS)
  end
  @host = "localhost"
  @port = 80
end
p Config.host                 # "localhost"
p Config.port                 # 80
Config.host = "example.com"
Config.port = 443
p Config.host                 # "example.com"
p Config.port                 # 443

# attr_accessor with a splat too
class Box
  FIELDS = [:w]
  class << self
    attr_accessor(*FIELDS)
  end
end
Box.w = 10
p Box.w                       # 10

# literal-symbol form still works alongside (unchanged path)
class Lit
  class << self
    attr_reader :name
  end
  @name = :lit
end
p Lit.name                    # :lit
