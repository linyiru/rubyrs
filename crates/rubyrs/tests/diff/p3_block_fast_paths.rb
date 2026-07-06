# Dispatch-campaign P3: block-form fast paths.
# 1) no_recv block IC (Object self): fixed-arity stack-direct, optional/
#    splat via the general invoke, private reach, toplevel fallback
#    (catch/throw), define_method closures, deny-listed names.
# 2) no_recv block IC (Class self): singleton resolution + toplevel
#    fallback from class-method bodies and module-scoped procs.
# 3) collection-receiver block serve: Array/Hash/Range native iterator
#    arms + Str gsub family, with reopen/subclass-override precedence,
#    frozen mutator errors, and break/next/non-local return semantics.

# --- 1. Object-self bare block calls -------------------------------
class Widget
  def fixed_two(a, b)
    yield(a + b) + 1
  end

  def optionalish(a, b = 10)
    yield(a + b)
  end

  def splatty(*xs)
    yield xs.sum
  end

  private def secret(n)
    yield n * 3
  end

  define_method(:dm_block) do |n, &blk|
    blk.call(n) * 2
  end

  def go
    r = []
    r << fixed_two(1, 2) { |s| s * 10 }
    r << optionalish(5) { |s| s + 100 }
    r << optionalish(5, 6) { |s| s + 100 }
    r << splatty(1, 2, 3) { |s| s - 1 }
    r << secret(7) { |s| s + 1 }
    r << dm_block(4) { |n| n + 5 }
    # toplevel-def fallback from an instance-method context
    r << catch(:tok) { |t| throw :tok, "caught-#{t.class}" }
    r << catch { |t| t.class.to_s }
    r
  end

  def super_probe
    each_hook { |x| x }
  end

  def each_hook
    yield "base"
  end
end

class SubWidget < Widget
  def each_hook
    "sub:" + super { |x| x }
  end
end

p Widget.new.go
p SubWidget.new.super_probe

# deny-listed names still behave (tap / lambda / proc / send family)
class DenyProbe
  def run
    a = []
    a << tap { |o| a << o.class.to_s }.class.to_s
    a << lambda { 1 }.call
    a << proc { 2 }.call
    a << send(:direct) { 3 }
    a << __send__(:direct) { 4 }
    a
  end

  def direct
    yield
  end
end
p DenyProbe.new.run

# method_missing fallback for a bare block call stays intact
class MM
  def method_missing(name, *args, &blk)
    "mm:#{name}:#{blk ? blk.call : "noblk"}"
  end

  def run
    ghost_call(1) { "b" }
  end
end
p MM.new.run

# &nil forwarding (run_callbacks shape): forwarded nil block re-aims
class NilFwd
  def outer(&blk)
    inner(&blk)
  end

  def inner
    block_given? ? "with" : "without"
  end
end
p NilFwd.new.outer
p NilFwd.new.outer { :x }

# --- 2. Class-self bare block calls --------------------------------
class Registrar
  def self.register(name)
    (@routes ||= []) << [name, yield]
    @routes.length
  end

  def self.routes = @routes

  def self.build
    n = register("a") { 1 }
    n += register("b") { 2 }
    # toplevel fallback with a Class self (the AS default_terminator shape)
    n += catch(:halt) { throw :halt, 10 }
    n
  end
end
p Registrar.build
p Registrar.routes

class SubRegistrar < Registrar
  def self.build_more
    register("c") { 3 } # inherited singleton via the class-self path
  end
end
p SubRegistrar.build_more

# module-scoped proc calling catch (self is the module/class object)
module TermFactory
  def self.terminator
    proc do |lam|
      stopped = true
      catch(:abort) do
        lam.call
        stopped = false
      end
      stopped
    end
  end
end
t = TermFactory.terminator
p t.call(proc { 1 })
p t.call(proc { throw :abort })

# bridge names keep their canonical route (class_eval in a class body)
class BridgeKeep
  class_eval do
    def via_eval = "ce"
  end
end
p BridgeKeep.new.via_eval

# --- 3. collection-receiver block serves ----------------------------
arr = [3, 1, 2]
p arr.each { |x| x }
acc = []
arr.each { |x| acc << x * 2 }
p acc
p arr.map { |x| x + 1 }
p arr.collect { |x| x * x }
p arr.select { |x| x > 1 }
p arr.filter(&:odd?)
p arr.reject { |x| x > 1 }
p arr.inject { |a, b| a + b }
p arr.inject(10) { |a, b| a + b }
p arr.reduce(2) { |a, b| a * b }
p arr.flat_map { |x| [x, x] }
p arr.detect { |x| x > 2 }
p arr.find { |x| x > 99 }
p arr.find_index { |x| x == 2 }
ewi = []
arr.each_with_index { |x, i| ewi << [i, x] }
p ewi
p arr.each_with_object([]) { |x, m| m << x }
slices = []
arr.each_slice(2) { |sl| slices << sl }
p slices
p arr.any? { |x| x > 2 }
p arr.all? { |x| x > 0 }
p arr.none? { |x| x > 5 }
p arr.min_by { |x| -x }
p arr.max_by { |x| -x }
p arr.sort_by { |x| -x }
p arr.group_by(&:odd?)
p arr.partition { |x| x > 1 }
p arr.count { |x| x > 1 }
p arr.sum { |x| x * 10 }
d = [1, 2, 3, 4]
d.delete_if { |x| x.even? }
p d
k = [1, 2, 3, 4]
k.keep_if { |x| x.even? }
p k

# break / next / non-local return through the served arms
p arr.each { |x| break "brk:#{x}" if x == 1 }
p arr.map { |x| next x * 100 if x == 1; x }
def nl_return(a)
  a.each { |x| return "ret:#{x}" if x == 2 }
  "fell"
end
p nl_return(arr)

h = { a: 1, b: 2 }
p h.each { |k2, v| [k2, v] }
hacc = []
h.each_pair { |k2, v| hacc << "#{k2}=#{v}" }
p hacc
p h.map { |k2, v| [k2, v * 2] }
p h.select { |_, v| v > 1 }
p h.reject { |_, v| v > 1 }
p(h.inject(0) { |s, (_, v)| s + v })
keys = []
h.each_key { |k2| keys << k2 }
p keys
vals = []
h.each_value { |v| vals << v }
p vals
p h.transform_keys(&:to_s)
p h.transform_values { |v| v + 10 }
p h.min_by { |_, v| v }
p h.sort_by { |_, v| -v }

r = (1..4)
p r.each { |x| x }
p r.map { |x| x * 2 }
p r.select(&:even?)
p r.inject { |a, b| a + b }
p r.inject(100) { |a, b| a + b }
p r.count(&:odd?)

s = "a_b_c"
p s.gsub(/_(\w)/) { $1.upcase }
p s.sub(/_(\w)/) { "-#{$1}-" }
p s.gsub("_") { "+" }
bang = +"x_y"
bang.gsub!(/_/) { "." }
p bang
p "ab12cd".scan(/\d/) { |m| }
lines = []
"l1\nl2\n".each_line { |l| lines << l.chomp }
p lines
chars = []
"xyz".each_char { |c| chars << c }
p chars

# frozen mutator errors come from the same arm
fr = [1, 2].freeze
begin
  fr.delete_if { true }
rescue FrozenError => e
  p e.class
end
fh = { x: 1 }.freeze
begin
  fh.delete_if { true }
rescue FrozenError => e
  p e.class
end
begin
  "frz".freeze.gsub!(/z/) { "Z" }
rescue FrozenError => e
  p e.class
end

# Hash-subclass override wins over the native arm (IndifferentHash shape)
class MyHash < Hash
  def select
    "overridden-select"
  end
end
mh = MyHash.new
mh[:k] = 1
p(mh.select { |_, v| v > 0 })

# Array-subclass override too
class MyArr < Array
  def map
    "overridden-map"
  end
end
ma = MyArr.new
ma << 1
p(ma.map { |x| x })

# String reopen wins over the native gsub arm (reopen-precedence gate)
class String
  def gsub(*args)
    "reopened-gsub"
  end
end
p "zzz".gsub(/z/) { "q" }

# new (non-shadowing) reopened names on collections still dispatch
class Array
  def frob
    "frob:#{length}:#{yield(first)}"
  end
end
p [9, 8].frob { |x| x + 1 }

# toplevel main-self bare block call (main is an Object self)
def main_helper(n)
  yield n + 1
end
p(main_helper(41) { |x| x })
p(catch(:main_tok) { throw :main_tok, :main_ok })
