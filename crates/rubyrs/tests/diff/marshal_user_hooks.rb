# `u` (_dump/_load) and `U` (marshal_dump/marshal_load) user hooks.
# A `_dump` result string's encoding flag depends on Integer#to_s's
# encoding (rubyrs UTF-8 vs CRuby US-ASCII), so the `u` case is tested
# by ROUND-TRIP; `U` (self-describing payload) is byte-comparable.
class Temp
  attr_reader :c
  def initialize(c); @c = c; end
  def _dump(level); @c.to_s; end
  def self._load(s); new(s.to_i); end
  def ==(o); o.is_a?(Temp) && o.c == c; end
end
r = Marshal.load(Marshal.dump(Temp.new(37)))
p [r.class.name, r.c, r == Temp.new(37)]
# binary _dump string → byte-comparable (no encoding wrapper)
class Bin
  def initialize(n); @n = n; end
  def _dump(l); [@n].pack("N"); end
  def self._load(s); new(s.unpack1("N")); end
  attr_reader :n
end
p Marshal.dump(Bin.new(5)).bytes
p Marshal.load(Marshal.dump(Bin.new(258))).n
# U: marshal_dump / marshal_load (byte-comparable, self-describing)
class Cfg
  attr_reader :h
  def initialize(h); @h = h; end
  def marshal_dump; @h; end
  def marshal_load(d); @h = d; end
end
c = Cfg.new({a: 1, b: [2, 3]})
p Marshal.dump(c).bytes
rc = Marshal.load(Marshal.dump(c))
p [rc.class.name, rc.h]
# deep copy independence through marshal_dump payload
orig = Cfg.new({list: [1, 2]})
copy = Marshal.load(Marshal.dump(orig))
copy.h[:list] << 9
p orig.h[:list]
# a raising _dump propagates the user exception (not TypeError/token)
class Boom; def _dump(l); raise ArgumentError, "no dump"; end; end
begin; Marshal.dump(Boom.new); rescue => e; p [e.class.name, e.message]; end
