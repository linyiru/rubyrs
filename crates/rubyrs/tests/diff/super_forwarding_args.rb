# `super(key, ...)` — Ruby 3.0 argument forwarding in an explicit-args
# super call (leading positional + `...`). Forwards positional, keyword,
# and block args. Surfaced by faraday's Utils::Headers#fetch.
class Base
  def fetch(key, default = :none)
    "B:#{key}/#{default}"
  end

  def opts(key, **kw)
    "B:#{key}/#{kw.sort.to_h}"
  end

  def blk(key)
    "B:#{key}/#{yield(10)}"
  end
end

class Child < Base
  def fetch(key, ...)
    super(key, ...)
  end

  def opts(key, ...)
    super(key, ...)
  end

  def blk(key, ...)
    super(key, ...)
  end
end

c = Child.new
p c.fetch(:a)
p c.fetch(:a, :b)
p c.opts(:a, x: 1, y: 2)
p c.blk(:a) { |n| n * 2 }
