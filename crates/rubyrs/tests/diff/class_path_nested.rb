# `class Foo::Bar` / `module Foo::Bar` — defining a nested
# class/module via constant-path syntax instead of physical
# nesting.
#
# Two equivalent shapes for the same outcome:
#
#   class Foo
#     class Bar; end           # physical nesting
#   end
#
#   class Foo; end
#   class Foo::Bar; end        # path-style (THIS fixture)
#
# CRuby treats both as `Foo::Bar`, with the second shape
# requiring that `Foo` already exists (NameError otherwise).
# rubyrs's spike-scope models classes by joined-name string
# in a flat `Vm.classes` table, so the path-style form lands
# under the same key as physical nesting would.
#
# Motivating use: MRI `lib/erb/compiler.rb:79`
# (`class ERB::Compiler` — at the top of the file with only
# `class ERB; end` ahead of it). Without this fixture,
# `Foo::Bar` reads after a `class Foo::Bar` body returned
# nil (`defined?(Foo::Bar)` reported "nil" rather than
# "constant").

# --- class Foo::Bar — basic ---
class Foo
end

class Foo::Bar
  GREETING = "hello"
  def self.hi
    "ok"
  end
end

puts Foo::Bar.hi                                # ok
puts Foo::Bar::GREETING                         # hello
puts Foo::Bar.name                              # Foo::Bar
puts defined?(Foo::Bar)                         # constant

# --- module Foo::Bar — same shape, module flavour ---
module ModA
end

module ModA::Inner
  def self.tag
    "AB"
  end
end

puts ModA::Inner.tag                            # AB
puts ModA::Inner.class                          # Module
puts ModA::Inner.name                           # ModA::Inner

# --- class Foo::Bar < Base — simple superclass via path-style class ---
class Animal
  def species; "generic"; end
end

class Zoo
end

class Zoo::Lion < Animal
  def species; "lion"; end
  def roar; "ROAR"; end
end

lion = Zoo::Lion.new
puts lion.species                               # lion
puts lion.roar                                  # ROAR
puts Zoo::Lion.superclass.name                  # Animal

# --- Multiple path-style classes under the same namespace ---
class Lib
end
class Lib::A
  def self.name_check; name; end
end
class Lib::B
  def self.name_check; name; end
end
class Lib::C
  def self.name_check; name; end
end
puts Lib::A.name_check                          # Lib::A
puts Lib::B.name_check                          # Lib::B
puts Lib::C.name_check                          # Lib::C

# --- Reopen a path-style class — methods accumulate ---
class Pkg
end
class Pkg::Tool
  def first; 1; end
end
class Pkg::Tool
  def second; 2; end
end
t = Pkg::Tool.new
puts t.first                                    # 1
puts t.second                                   # 2

# --- ERB-shape probe ---
# Mirror MRI's lib/erb.rb / lib/erb/compiler.rb structure: an
# outer `class ERB` shell, then a path-style `class ERB::Compiler`
# (at top level, NOT physically nested) that picks up the outer
# shell. ERB::Compiler defines its own self.compile entry. This
# is the exact shape that motivated the gap.
class ERBShim
end

class ERBShim::Compiler
  def initialize(trim_mode)
    @trim_mode = trim_mode
  end
  def trim_mode
    @trim_mode
  end
end

c = ERBShim::Compiler.new("-")
puts c.trim_mode                                # -
puts ERBShim::Compiler.name                     # ERBShim::Compiler
