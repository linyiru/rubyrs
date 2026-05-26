# `Module#include?` accepts only Module args in CRuby —
# passing a Class raises TypeError. The previous
# behaviour (`module_introspection.rb` documented this
# divergence) accepted both because rubyrs's Class struct
# backs both shapes. Now closed via the `is_module`
# flag added in `f74feb8`.
#
# CRuby's error message uses the receiver's *type* name
# ("Class"), not the class's identity name —
# `Wrap.include?(C)` says
# `wrong argument type Class (expected Module)`, NOT
# `wrong argument type C (expected Module)`. The fixture
# pins this exact shape.

module M
end

class C
end

module Wrap
  include M
end

# Module arg — succeeds.
puts Wrap.include?(M)

# Class arg — TypeError with CRuby's exact message.
begin
  Wrap.include?(C)
rescue TypeError => e
  puts "TypeError: #{e.message}"
end

# Non-Module/Class arg — also TypeError. CRuby's message
# varies by arg type ("String" / "Integer" / ...); the
# fixture only checks the error class to stay
# cross-implementation portable.
begin
  Wrap.include?("not a module")
rescue TypeError
  puts "TypeError caught for String"
end

begin
  Wrap.include?(42)
rescue TypeError
  puts "TypeError caught for Integer"
end

begin
  Wrap.include?(nil)
rescue TypeError
  puts "TypeError caught for nil"
end

# Stdlib stub kind agrees with this check — URI is a
# Module so `Wrap.include?(URI)` doesn't raise the type
# error (it just returns false because Wrap doesn't
# include URI).
require 'uri'
puts Wrap.include?(URI)
