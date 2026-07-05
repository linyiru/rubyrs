# `class << Const; ...; end` — the LITERAL eigenclass body on a
# constant (and wider non-self) receiver must reach the same
# machinery as the call form `Const.singleton_class.include(M)`
# (M5, 0cb50579: shell redirect into real.singleton_includes /
# singleton_prepends). Pre-lift, two body shapes hit a parse-time
# SyntaxError when nothing else in the body routed it to the real
# eigenclass path:
#   - bare `prepend Mod` (the desugar's SingletonChainPrepend is
#     `class << self`-only)
#   - `attr_*` with splat / non-Symbol args (the desugar's non-self
#     attr arm only expands plain-Symbol lists)
# Both now route whole-body to the real eigenclass body
# (Op::OpenSingletonClass, self = the metaclass shell).
#
# Documented divergences NOT pinned here (all pre-existing, shared
# with `class << self`, NOT part of the parse-level lift):
#   - eigenclass-scoped constants leak to the top-level const table
#     (flat const model): `Object.const_get(:EIGC)` finds it where
#     CRuby raises NameError. The working part IS pinned: bare reads
#     from body methods and `Const.singleton_class::EIGC`.
#   - `private_constant` inside the eigenclass body is a no-op.
#   - `def self.x` inside the body (double eigenclass) NoMethodErrors.
#   - bare `protected` inside the body is a no-op.

# --- prepend-only body: THE lifted shape -----------------------------
module TagPre
  def tag
    "pre+" + super
  end
end

class Widget
  def self.tag
    "base"
  end
end

class << Widget
  prepend TagPre
end
p Widget.tag
p Widget.singleton_class.ancestors.include?(TagPre)

# --- multi-arg prepend, right-to-left insertion ----------------------
module PA; def pa; "pa"; end; end
module PB; def pb; "pb"; end; end
class Multi; end
class << Multi
  prepend PA, PB
end
p [Multi.pa, Multi.pb]

# --- attr_* splat: the other lifted shape ----------------------------
ATTRS = [:width, :height].freeze
class Board; end
class << Board
  attr_accessor(*ATTRS)
end
Board.width = 3
Board.height = 4
p [Board.width, Board.height]

# attr_* with a String arg (CRuby accepts, defines :sa)
class Strung; end
class << Strung
  attr_accessor "sa"
end
Strung.sa = 5
p Strung.sa

# --- full mixed body against one receiver ----------------------------
module Helper
  def helped
    "helped:#{label}"
  end
end
module Wrap
  def describe
    "[" + super + "]"
  end
end

class Gadget
  def self.label
    "Gadget"
  end
  def self.describe
    "plain"
  end
end

class << Gadget
  include Helper
  prepend Wrap
  def kls
    "kls"
  end
  attr_accessor :cfg
  alias_method :also_describe, :describe
  define_method(:dm) { "dm" }
  p method_defined?(:label)
  private def secret
    "s3"
  end
end

p Gadget.helped
p Gadget.describe
p Gadget.kls
Gadget.cfg = 42
p Gadget.cfg
p Gadget.also_describe
p Gadget.dm
begin
  Gadget.secret
rescue NoMethodError
  puts "secret private: ok"
end
p Gadget.send(:secret)
# include went to the metaclass — instances unaffected
begin
  Gadget.new.helped
rescue NoMethodError
  puts "instance NoMethodError: ok"
end

# --- visibility: bare private, private :name -------------------------
class Vis
  def self.shown
    "shown"
  end
end
class << Vis
  private
  def hidden
    "hid"
  end
end
begin
  Vis.hidden
rescue NoMethodError
  puts "hidden private: ok"
end
p Vis.send(:hidden)

class << Vis
  private :shown
end
begin
  Vis.shown
rescue NoMethodError
  puts "shown made private: ok"
end

# --- eigenclass-scoped constant: the working surface ------------------
class Consty; end
class << Consty
  EIGC = 41
  def rd
    EIGC
  end
end
p Consty.rd
p Consty.singleton_class::EIGC

# --- extend inside the body: eigenclass-object semantics --------------
module Ext
  def em
    "em"
  end
end
class Exty; end
class << Exty
  extend Ext
end
p Exty.singleton_class.em
p Exty.respond_to?(:em)

# --- nested class << self inside -------------------------------------
class Nesty; end
class << Nesty
  class << self
    def nested
      "nested"
    end
  end
end
p Nesty.singleton_class.nested

# --- nested module definition + include ------------------------------
class Roomy; end
class << Roomy
  module Inner
    def im
      "im"
    end
  end
  include Inner
end
p Roomy.im

# --- construct value: `class << Const; self; end` ---------------------
class Valued; end
mc = class << Valued
  self
end
p mc == Valued.singleton_class
p(class << Valued; end)

# --- module receiver ---------------------------------------------------
module Registry; end
class << Registry
  prepend TagPre
  def tag
    "reg"
  end
end
p Registry.tag

# --- non-Const receivers: local var and expression ---------------------
obj = Object.new
class << obj
  prepend Module.new {
    def hi
      "wrapped " + super
    end
  }
  def hi
    "hi-obj"
  end
end
p obj.hi

def maker
  $made = Object.new
end
class << maker
  def made_hi
    "made"
  end
end
p $made.made_hi

# --- qualified-const receiver: class << NS::Deep -----------------------
module NS
  class Deep
    def self.tag
      "deep"
    end
  end
end
class << NS::Deep
  prepend TagPre
  def extra
    "extra"
  end
end
p NS::Deep.tag
p NS::Deep.extra

# --- singleton chains survive later defs (method_gen) ------------------
class Late; end
class << Late
  prepend TagPre
end
def Late.tag
  "late-base"
end
p Late.tag
