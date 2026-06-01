# Bare method calls inside reopened primitive classes — the
# pattern every `to_json` / `as_json` mixin (and ActiveSupport-
# style core-ext slice) leans on. Pre-fix, bare-call dispatch
# inside a `class Integer; def x; sibling; end; end` shape
# raised "undefined method `sibling' for Integer" because
# do_call's no_recv path only consulted the primitive's class
# for `respond_to?` specifically. The general lookup arm now
# walks `class_of(self_val)` for all primitive selves; primitive
# built-ins (Int#to_s, Str#length, …) reach the receiver-form
# arm via the bridge fallback when the Ruby-level method table
# misses.

# --- sibling user-defined methods (Integer / Float / String / Symbol) ---
class Integer
  def my_helper; "int-helper-#{self}"; end
  def calls_helper; my_helper; end
end
puts 42.calls_helper

class Float
  def my_helper; "float-helper-#{self}"; end
  def calls_helper; my_helper; end
end
puts 1.5.calls_helper

class String
  def my_helper; "str-helper-#{self}"; end
  def calls_helper; my_helper; end
end
puts "abc".calls_helper

class Symbol
  def my_helper; "sym-helper-#{self}"; end
  def calls_helper; my_helper; end
end
puts :foo.calls_helper

# --- bare-call resolving to a PRIMITIVE built-in (e.g. Int#to_s) ---
class Integer
  def shout_via_bare
    to_s + "!"
  end
end
puts 42.shout_via_bare

class String
  def echo_size
    length
  end
end
puts "hello".echo_size

# --- toplevel def + arity mismatch still raises ArgumentError
# (not NoMethodError for NilClass — guards the regression we
# nearly shipped while developing the primitive bridge: Nil-self
# is the toplevel sentinel and must NOT bridge to NilClass
# method lookup).
def one_only(a); a; end
begin
  one_only(1, 2)
  puts "should-not-reach"
rescue ArgumentError
  puts "argerr"
end
