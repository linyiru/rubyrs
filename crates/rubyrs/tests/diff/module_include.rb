# `module Foo; def bar; ...; end; end` + `include Foo` — proper
# mixin via the includes chain. Method lookup now walks
# `class own methods → included modules (reverse-include order) →
# superclass`, instead of the previous "copy methods at include
# time" approximation.
#
# Wins from the chain model:
#   1. Methods added to the module AFTER include propagate to the
#      including class (the chain is read live at dispatch).
#   2. `is_a?(Mod)` / `kind_of?(Mod)` work for the included
#      module.
#   3. `Class#ancestors` shows the full chain.

module Greetable
  def hello
    "hello, #{name}"
  end
end

class Person
  include Greetable
  def initialize(name)
    @name = name
  end
  def name
    @name
  end
end

p = Person.new("Mochi")
puts p.hello                              # hello, Mochi
puts p.is_a?(Greetable)                   # true
puts p.is_a?(Person)                      # true

# is_a? + kind_of? aliases.
puts p.kind_of?(Greetable)                # true

# Class#include? — direct query.
puts Person.include?(Greetable)           # true

# Multiple includes, last-wins on method conflict.
module FirstHello
  def greet
    "first"
  end
end
module SecondHello
  def greet
    "second"
  end
end
class Conflicting
  include FirstHello
  include SecondHello
end
puts Conflicting.new.greet                # second (last include wins)

# Methods added to the module AFTER include are visible.
module Augmentable
end
class Augmented
  include Augmentable
end
module Augmentable
  def added_later
    "still works"
  end
end
puts Augmented.new.added_later

# rescue against an included module works (class_is_a follows chain).
module MyError
end
class SpecificError < StandardError
  include MyError
end
begin
  raise SpecificError, "boom"
rescue MyError => e
  puts "caught via included module: #{e.class.name}"
end
