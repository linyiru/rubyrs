# The per-site super cache also records misses (`super` with no method above).
# A cached miss must turn into a hit once an ancestor gains the method, and
# the deferred "no superclass method" error keeps its message and backtrace.
class Base; end
class Kid < Base
  def greet; super; end
end
k = Kid.new
3.times do
  begin
    k.greet
  rescue NoMethodError => e
    puts e.message.tr("`", "'")
    p e.backtrace.first[/:(\d+):/, 1]
  end
end
class Base
  def greet; "base greet"; end
end
3.times { p k.greet }

# Miss, then hit through a module included into the superclass.
module Hello
  def hello; "hello from module"; end
end
class Kid
  def hello; super; end
end
2.times { p((k.hello rescue "miss")) }
Base.include(Hello)
2.times { p k.hello }

# Remove the method again: the site misses once more.
class Base
  remove_method :greet
end
2.times { p((k.greet rescue "miss again")) }

# Builtin substitutions served off a cached miss, repeatedly.
class Sub
  def initialize(x); @x = x; super(); end
  def respond_to?(m, p = false); m == :magic || super; end
  def is_a?(k); k == :fake || super; end
  def freeze; @froze = true; super; end
  attr_reader :x, :froze
end
5.times do |i|
  s = Sub.new(i)
  p [s.x, s.respond_to?(:magic), s.respond_to?(:x), s.respond_to?(:nope),
     s.is_a?(Sub), s.is_a?(:fake), s.is_a?(String), s.freeze.frozen?, s.froze]
end

# Block-form super (Op::ApplySuperBlock) miss, then hit.
class Kid
  def each_thing(&b); super(&b); end
end
2.times { p((k.each_thing { 1 } rescue "block miss")) }
class Base
  def each_thing; yield + 1; end
end
2.times { p k.each_thing { 1 } }

# Class-method super miss keeps its receiver description.
class Kid
  def self.make; super; end
end
2.times do
  Kid.make
rescue NoMethodError => e
  puts e.message.tr("`", "'")
end
