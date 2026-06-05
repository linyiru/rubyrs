# `Forwardable` + `SingleForwardable` shim — minimal surface
# covering `extend Forwardable`/`SingleForwardable` followed
# by class-body `def_delegator(s)` / `single_delegate` calls.
# Pre-shim, these tripped `NoMethodError: undefined method
# 'def_delegators' for Class` at module-load time, blocking
# every Sinatra / Mustermann / Rack `require`.

require "forwardable"

# 1. Forwardable: `def_delegators :@ivar, *methods` — the
# Rack 3 `Rack::Lint::Wrapper` pattern.
class W1
  extend Forwardable
  def initialize(inner)
    @inner = inner
  end
  def_delegators :@inner, :size, :first, :last
end
w = W1.new([10, 20, 30])
puts "w_size=#{w.size}"
puts "w_first=#{w.first}"
puts "w_last=#{w.last}"

# 2. Forwardable: `def_delegator :@ivar, :method, :alias` —
# 3-arg form aliases the delegate name.
class W2
  extend Forwardable
  def initialize(arr); @arr = arr; end
  def_delegator :@arr, :length, :len
  def_delegator :@arr, :[], :at
end
w2 = W2.new([1, 2, 3, 4])
puts "len=#{w2.len}"
puts "at=#{w2.at(2)}"

# 3. Forwardable: `def_delegators :reader, ...` — Symbol
# accessor that names a READER METHOD (not an ivar). Same
# Mustermann shape from `mustermann/ast/parser.rb:45`.
class W3
  extend Forwardable
  attr_reader :buffer
  def initialize(buf); @buffer = buf; end
  def_delegators :buffer, :size, :empty?
end
w3 = W3.new([])
puts "via_reader_empty=#{w3.empty?}"
puts "via_reader_size=#{w3.size}"

# 4. Delegation to method that takes an arg — verifies the
# `*args` splat forwarding works (no block, no kwargs).
# Block-forwarding through `&blk` works correctly under normal
# mode but exposes an unrelated GC root-hole in rubyrs's block
# capture under STRESS_GC=1 (the captured `&blk` ObjId is
# swept between dispatch hops); covered in the regular
# probe_fwd.rb smoke test but kept out of the diff_cruby
# scenarios to avoid flapping that orthogonal divergence.
class W4
  extend Forwardable
  def initialize(arr); @arr = arr; end
  def_delegator :@arr, :include?, :has?
end
w4 = W4.new([10, 20, 30])
puts "has_yes=#{w4.has?(20)}"
puts "has_no=#{w4.has?(99)}"

# 5. SingleForwardable hash-kwarg form — Mustermann's
# `single_delegate on: :parser, suffix: :parser` shape from
# `mustermann/ast/pattern.rb:22`.
class W5
  class << self
    attr_accessor :parser
  end
  extend SingleForwardable
  single_delegate on: :parser, suffix: :parser
end
class Parser5
  def on; "parser-on"; end
  def suffix; "parser-suffix"; end
end
W5.parser = Parser5.new
puts "single_on=#{W5.on}"
puts "single_suffix=#{W5.suffix}"

# 6. SingleForwardable: `def_single_delegator(accessor,
# method, alias)` — positional form aliases the delegate.
class W6
  class << self
    attr_accessor :counter
  end
  extend SingleForwardable
  def_single_delegator :counter, :+, :bump
end
class Counter6
  def initialize; @n = 0; end
  def +(other); @n += other; end
end
W6.counter = Counter6.new
puts "single_bump=#{W6.bump(5)}"
puts "single_bump_again=#{W6.bump(7)}"
