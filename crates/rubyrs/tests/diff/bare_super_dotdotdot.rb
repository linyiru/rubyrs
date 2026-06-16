# Bare `super` inside a `def m(...)` argument-forwarding method must
# forward the anonymous rest/kwrest/block exactly like `super(...)` —
# not pass the `*` rest array as a single positional. Surfaced by
# signalize's `def self.signal_accessor(...); super; ...` (Bridgetown
# Site.new path).
class Base
  def acc(*names, **opts, &blk)
    names.each { |n| puts "p:#{n.to_sym}" }
    opts.each { |k, v| puts "k:#{k}=#{v}" }
    puts "b:#{blk.call}" if blk
  end
  def self.cacc(*names)
    names.each { |n| puts "cp:#{n}" }
  end
end
class Sub < Base
  def acc(...)
    super
  end
  def self.cacc(...)
    super
  end
end
Sub.new.acc(:x, :y, a: 1) { "yes" }
Sub.cacc(:m, :n)
