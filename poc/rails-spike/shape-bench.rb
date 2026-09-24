# Per-operation cost of the shapes a Rails request is made of (see the
# CRuby TracePoint census: ~1.4k Ruby calls, ~1.7k C calls, ~220 blocks,
# ~430 allocations per GET). Each case is a `while` loop in its own
# method; the empty-loop cost is subtracted, so the printed number is
# ns per operation. Same file runs on CRuby and rubyrs.
#
#   ruby --yjit poc/rails-spike/shape-bench.rb
#   target/release/rubyrs poc/rails-spike/shape-bench.rb
#
# SCALE (default 1.0) multiplies every iteration count.

SCALE = Float(ENV["SCALE"] || 1.0)

class Base
  def initialize(a, b); @a = a; @b = b; end
  def greet(x) = x
end

class Obj < Base
  attr_reader :a
  def foo; end
  def two(x, y); end
  def kw(a:, b: 2); end
  def kwsplat(**o); end
  def optpos(a, b = 1, *rest); end
  def y; yield; end
  def y1; yield 1; end
  def blk(&b) = b.call
  def greet(x) = super
  def ivar_rw; @a = @a; end
  define_method(:dm) { }
  def method_missing(name, *args)
    name.end_with?("=") ? args.first : name
  end
  def respond_to_missing?(n, p = false) = true
end

class Cfg
  class << self
    define_method(:__class_attr_config) { 1 }
    private :__class_attr_config
    def config = __class_attr_config
  end
end
module Mixin; end
class Obj; include Mixin; end

O = Obj.new(1, 2)
H = { a: 1, b: 2, "k" => 3, c: 4 }
ARR10 = (1..10).to_a
STR = "/hello/world"
RE = /\A\/hello/
MTX = Mutex.new
PROC = proc { |x| x }
LAM = ->(x) { x }
FILE_PATH = __FILE__

def empty_loop(n); i = 0; while i < n; i += 1; end; end

CASES = {
  "call 0-arg"            => [1.0, ->(n) { o = O; i = 0; while i < n; o.foo; i += 1; end }],
  "call 2-arg"            => [1.0, ->(n) { o = O; i = 0; while i < n; o.two(1, 2); i += 1; end }],
  "call kwargs a:,b:"     => [1.0, ->(n) { o = O; i = 0; while i < n; o.kw(a: 1, b: 2); i += 1; end }],
  "call **opts"           => [1.0, ->(n) { o = O; i = 0; while i < n; o.kwsplat(a: 1); i += 1; end }],
  "call opt+rest"         => [1.0, ->(n) { o = O; i = 0; while i < n; o.optpos(1); i += 1; end }],
  "attr_reader"           => [1.0, ->(n) { o = O; i = 0; while i < n; o.a; i += 1; end }],
  "ivar read+write"       => [1.0, ->(n) { o = O; i = 0; while i < n; o.ivar_rw; i += 1; end }],
  "super (1 level)"       => [1.0, ->(n) { o = O; i = 0; while i < n; o.greet(1); i += 1; end }],
  "define_method call"    => [1.0, ->(n) { o = O; i = 0; while i < n; o.dm; i += 1; end }],
  "yield"                 => [1.0, ->(n) { o = O; i = 0; while i < n; o.y { }; i += 1; end }],
  "yield 1 arg"           => [1.0, ->(n) { o = O; i = 0; while i < n; o.y1 { |x| x }; i += 1; end }],
  "&blk + blk.call"       => [1.0, ->(n) { o = O; i = 0; while i < n; o.blk { }; i += 1; end }],
  "proc.call"             => [1.0, ->(n) { pr = PROC; i = 0; while i < n; pr.call(1); i += 1; end }],
  "lambda.()"             => [1.0, ->(n) { l = LAM; i = 0; while i < n; l.(1); i += 1; end }],
  "method_missing"        => [0.5, ->(n) { o = O; i = 0; while i < n; o.nope; i += 1; end }],
  "send(:foo)"            => [1.0, ->(n) { o = O; i = 0; while i < n; o.send(:foo); i += 1; end }],
  "respond_to?"           => [1.0, ->(n) { o = O; i = 0; while i < n; o.respond_to?(:foo); i += 1; end }],
  "is_a?(Module)"         => [1.0, ->(n) { o = O; i = 0; while i < n; o.is_a?(Mixin); i += 1; end }],
  "Module#=== (case)"     => [1.0, ->(n) { o = O; i = 0; while i < n; case o when String then 1 when Base then 2 end; i += 1; end }],
  "class_attr reader"     => [1.0, ->(n) { i = 0; while i < n; Cfg.config; i += 1; end }],
  "obj.class"             => [1.0, ->(n) { o = O; i = 0; while i < n; o.class; i += 1; end }],
  "Class#new(2) +init"    => [0.5, ->(n) { i = 0; while i < n; Base.new(1, 2); i += 1; end }],
  "Hash#[] sym"           => [1.0, ->(n) { h = H; i = 0; while i < n; h[:b]; i += 1; end }],
  "Hash#[] str"           => [1.0, ->(n) { h = H; i = 0; while i < n; h["k"]; i += 1; end }],
  "Hash#[]= sym"          => [1.0, ->(n) { h = {}; i = 0; while i < n; h[:x] = i; i += 1; end }],
  "Hash#fetch"            => [1.0, ->(n) { h = H; i = 0; while i < n; h.fetch(:a, nil); i += 1; end }],
  "Hash literal (4)"      => [0.5, ->(n) { i = 0; while i < n; { a: i, b: 2, c: 3, d: 4 }; i += 1; end }],
  "Hash#dup (4)"          => [0.5, ->(n) { h = H; i = 0; while i < n; h.dup; i += 1; end }],
  "Hash#merge (4+1)"      => [0.5, ->(n) { h = H; i = 0; while i < n; h.merge(z: 1); i += 1; end }],
  "Array#each (10)"       => [0.2, ->(n) { a = ARR10; i = 0; while i < n; a.each { |x| x }; i += 1; end }],
  "Array#map (10)"        => [0.2, ->(n) { a = ARR10; i = 0; while i < n; a.map { |x| x }; i += 1; end }],
  "Array#any? blk (10)"   => [0.2, ->(n) { a = ARR10; i = 0; while i < n; a.any? { |x| x > 20 }; i += 1; end }],
  "Array literal (3)"     => [1.0, ->(n) { i = 0; while i < n; [i, 2, 3]; i += 1; end }],
  "String interp"         => [0.5, ->(n) { i = 0; while i < n; "a#{i}b"; i += 1; end }],
  "String#==(literal)"    => [1.0, ->(n) { s = STR; i = 0; while i < n; s == "/hello/world"; i += 1; end }],
  "String#start_with?"    => [1.0, ->(n) { s = STR; i = 0; while i < n; s.start_with?("/"); i += 1; end }],
  "String#dup"            => [0.5, ->(n) { s = STR; i = 0; while i < n; s.dup; i += 1; end }],
  "String#downcase"       => [0.5, ->(n) { s = "Content-Type"; i = 0; while i < n; s.downcase; i += 1; end }],
  "Regexp =~ literal"     => [0.5, ->(n) { s = STR; i = 0; while i < n; s =~ RE; i += 1; end }],
  "String#match?(re)"     => [0.5, ->(n) { s = STR; i = 0; while i < n; s.match?(RE); i += 1; end }],
  "String#sub(str,str)"   => [0.5, ->(n) { s = STR; i = 0; while i < n; s.sub("hello", "x"); i += 1; end }],
  "String#split('/')"     => [0.2, ->(n) { s = STR; i = 0; while i < n; s.split("/"); i += 1; end }],
  "Symbol#to_s"           => [1.0, ->(n) { i = 0; while i < n; :foo.to_s; i += 1; end }],
  "Symbol#end_with?"      => [1.0, ->(n) { i = 0; while i < n; :foo.end_with?("="); i += 1; end }],
  "Thread.current[:k]"    => [1.0, ->(n) { i = 0; while i < n; Thread.current[:k]; i += 1; end }],
  "Mutex#synchronize"     => [0.5, ->(n) { m = MTX; i = 0; while i < n; m.synchronize { }; i += 1; end }],
  "clock_gettime"         => [0.5, ->(n) { i = 0; while i < n; Process.clock_gettime(Process::CLOCK_MONOTONIC); i += 1; end }],
  "begin/ensure"          => [1.0, ->(n) { i = 0; while i < n; begin; i; ensure; i; end; i += 1; end }],
  "raise+rescue"          => [0.05, ->(n) { i = 0; while i < n; begin; raise ArgumentError; rescue ArgumentError; end; i += 1; end }],
  "File.mtime"            => [0.05, ->(n) { f = FILE_PATH; i = 0; while i < n; File.mtime(f); i += 1; end }],
}

BASE_N = (300_000 * SCALE).to_i
def time_ms
  t0 = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  yield
  (Process.clock_gettime(Process::CLOCK_MONOTONIC) - t0) * 1e9
end

# warm everything once (IC fill, JIT thresholds)
CASES.each_value { |f, l| l.call((20_000 * f).to_i) }
empty_ns = [3].map { time_ms { empty_loop(BASE_N) } }.min / BASE_N

CASES.each do |name, (f, l)|
  n = (BASE_N * f).to_i
  best = 3.times.map { time_ms { l.call(n) } }.min
  puts format("%-22s %9.1f", name, best / n - empty_ns)
end
puts format("%-22s %9.1f", "(empty loop iter)", empty_ns)
