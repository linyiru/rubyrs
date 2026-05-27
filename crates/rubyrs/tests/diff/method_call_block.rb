# `BoundMethod#call(args, &block)` — block-form Method invocation.
#
# `obj.method(:name)` returns a Method (BoundMethod in rubyrs's
# internal representation); calling it with `m.call(args)` was
# already supported (do_call's BoundMethod arm at dispatch.rs:1969),
# but the block-form `m.call(args) { ... }` and `m.call(args, &block)`
# fell through `do_call_block` to NoMethodError because that path
# had no Method-recv arm.
#
# Motivating use: MRI's `lib/erb/compiler.rb:147`
#   @scan_line.call(@src, &block)
# where `@scan_line` was cached at object-construction time via
# `self.method(:simple_scan)` etc. Without this fix the ERB
# compile body raises immediately.

# --- Basic: bm.call with an inline block ---
class C
  def with_block(name, &block)
    block.call(name)
  end
end
m = C.new.method(:with_block)
m.call("X") { |x| puts "got #{x}" }              # got X

# --- &block forwarding ---
# The caller's own block forwarded as an &-arg.
class D
  def each_letter(&block)
    %w[a b c].each(&block)
  end
end
d = D.new
m2 = d.method(:each_letter)
collected = []
m2.call { |l| collected << l }
puts collected.inspect                           # ["a", "b", "c"]

# --- m.call(&block) — no positional args, just the block ---
class E
  def call_block(&b)
    b.call(:hello)
  end
end
m3 = E.new.method(:call_block)
m3.call { |sym| puts sym }                       # hello

# --- `[]` and `()` are aliases for call ---
# CRuby allows `m[args]` and `m.()` for method invocation; both
# must accept blocks in the same way.
class F
  def take_with_block(n, &b)
    b.call(n * 2)
  end
end
mf = F.new.method(:take_with_block)
mf[3] { |r| puts "alias[] #{r}" }                # alias[] 6
mf.(4) { |r| puts "alias() #{r}" }               # alias() 8

# --- Method on a singleton ---
class G; end
g = G.new
def g.private_singleton(x, &b)
  b.call(x.upcase)
end
mg = g.method(:private_singleton)
mg.call("hi") { |s| puts s }                     # HI

# --- ERB-shape probe ---
# Cache a method to an ivar then invoke it with a block — the
# exact shape lib/erb/compiler.rb:147 uses (Scanner caches
# `@scan_line = method(:scan_line_impl)` at construction time
# and later calls `@scan_line.call(@src, &block)` from `scan`).
class ScannerLike
  def initialize
    @scan_method = self.method(:scan_impl)
  end
  def scan_impl(items, &block)
    items.each { |x| block.call(x.upcase) }
  end
  def scan(items, &block)
    @scan_method.call(items, &block)
  end
end
out = []
ScannerLike.new.scan(%w[a b c]) { |line| out << line }
puts out.inspect                                 # ["A", "B", "C"]
