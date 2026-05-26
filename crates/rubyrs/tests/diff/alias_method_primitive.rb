# alias_method targeting a primitive method. Previously
# raised NameError because primitive methods
# (`Symbol#name`, `Integer#+`, `String#upcase` etc.) live in
# rubyrs's per-Value primitive-call whitelist rather than
# the user-Method table that `alias_method` consults. Now
# the AliasMethod handler synthesises a forwarder Method
# whose body re-dispatches `old_id` on `self`, so the alias
# behaves exactly like a direct primitive call (variadic-
# arg forwarding via the rest-Array slot).
#
# Surfaced by msgpack-ruby `lib/msgpack/symbol.rb`'s
# `alias_method :to_msgpack_ext, :name` shape — Symbol#name
# is a primitive (Ruby-3+ shape, landed in `a5fd683`).

# 0-arity primitive alias.
class Symbol
  alias_method :to_msgpack_ext, :name
end
puts :foo.to_msgpack_ext              # "foo"
puts :hello_world.to_msgpack_ext      # "hello_world"

# Variadic primitive alias — args forwarded via the rest
# slot. Integer#+ takes one arg; the alias should accept the
# same arity transparently.
class Integer
  alias_method :plus, :+
  alias_method :times_by, :*
end
puts 3.plus(4)                        # 7
puts 5.times_by(6)                    # 30
puts 100.plus(-50)                    # 50

# 0-arity transformer.
class String
  alias_method :shout, :upcase
  alias_method :flip, :reverse
end
puts "hello".shout                    # "HELLO"
puts "rubyrs".flip                    # "srybur"

# Bare-call form inside a class body resolving alias_method
# directly (vs. through send).
class Float
  alias_method :ceiling, :ceil
end
puts 3.14.ceiling                     # 4

# Alias resolves through normal dispatch — the synthesised
# forwarder works for both the receiver form and any path
# that goes through do_call on the same recv.
class Symbol
  alias_method :show, :to_s
end
puts :bar.show                        # "bar"
[:a, :b, :c].each { |s| puts s.show }
