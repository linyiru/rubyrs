# Class variables `@@foo` — read, write, and the three
# operator-write forms (`+=`, `||=`, `&&=`). Previously
# raised `unsupported node: ClassVariableReadNode /
# ClassVariableWriteNode` at AST translation.
#
# Surfaced while probing Sinatra `lib/sinatra/show_exceptions.rb`
# for load coverage — its `@@eats_errors = Logger.new(...)` +
# `@@eats_errors.warn(...)` shape is the canonical
# "module-level cache" use case for class variables across
# Sinatra, Rails internals, dry-struct, tilt, etc.
#
# Documented Tier 1 divergence (NOT exercised by this
# fixture): rubyrs stores `@@foo` on the surrounding class
# only — no walk-up-the-hierarchy on read or write. CRuby's
# semantics would alias `@@foo` across a class and its
# descendants; we'd need a hierarchy walk to model that.
# Mainstream uses keep `@@foo` on a single class, which is
# what this fixture verifies.

class Counter
  @@count = 0

  def self.bump
    @@count += 1
  end

  def self.value
    @@count
  end

  def add_n(n)
    @@count += n
  end
end

# Class-method writes.
puts Counter.value                 # 0
Counter.bump
Counter.bump
puts Counter.value                 # 2

# Instance-method writes share the same `@@count`.
Counter.new.add_n(10)
puts Counter.value                 # 12

# `||=` — initialise on first hit.
class Cache
  @@hits = nil

  def self.hits
    @@hits ||= 0
    @@hits += 1
    @@hits
  end
end
puts Cache.hits                    # 1
puts Cache.hits                    # 2
puts Cache.hits                    # 3

# `&&=` — refresh-if-set.
class Token
  @@value = "abc"

  def self.refresh!
    @@value &&= @@value.upcase
  end

  def self.value
    @@value
  end
end
puts Token.value                   # "abc"
Token.refresh!
puts Token.value                   # "ABC"

# Read inside a block evaluated in the surrounding method
# also works (block frame inherits self_val).
class Tally
  @@items = []

  def self.tally(*xs)
    xs.each { |x| @@items << x }
    @@items
  end
end
Tally.tally(:a, :b, :c)
puts Tally.tally(:d).inspect       # [:a, :b, :c, :d]

# Toplevel `@@foo` — Tier 1 lenient fallback to
# Vm.toplevel_cvars (CRuby raises RuntimeError; rubyrs
# allows it as a script-level cache like ivars / globals).
# Documented divergence; CRuby parity check skipped here
# rather than asserted to keep the fixture portable.
