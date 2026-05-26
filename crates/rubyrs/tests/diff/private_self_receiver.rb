# `self.private_method` — CRuby 2.7+ allows explicit-self
# receivers to dispatch private methods. Pre-2.7 only allowed
# setter-via-self (`self.foo = v`); modern Ruby broadened the
# exception to any explicit `self.x`.
#
# The rule: when dispatching to a private method, an explicit
# receiver is allowed iff that receiver is identity-equal to
# the caller's self.
#
# Motivating use: MRI's `lib/erb/compiler.rb:328`
#   self.content = +''
# where `:content` is declared `attr_accessor` then `private`.
# The `self.content =` form has to work; the bare `content =`
# would parse as a local-variable assignment (which is the
# other half of why the language requires this exception).

class Acc
  attr_accessor :balance
  private :balance, :balance=
  def initialize
    @balance = 0
  end

  # Getter via self works:
  def via_self_getter
    self.balance
  end

  # Setter via self works:
  def via_self_setter(v)
    self.balance = v
  end

  # Bare-form setter parses as local variable assignment in
  # CRuby (a documented Ruby quirk — NOT the method call).
  # The ivar stays untouched.
  def bare_setter_local(v)
    balance = v   # local var, not call
    balance       # reads the local
  end
end

a = Acc.new
puts a.via_self_getter                          # 0
a.via_self_setter(100)
puts a.via_self_getter                          # 100

# Bare-form returns the local var value but doesn't touch
# the ivar. This is the parser ambiguity rule, not visibility.
puts a.bare_setter_local("local-only")          # local-only
puts a.via_self_getter                          # 100 (unchanged)

# --- External call MUST still fail ---
# The exception is ONLY for explicit-self; outside callers
# still hit the private barrier.
begin
  a.balance = 999
rescue NoMethodError
  puts "extern setter denied"
end
begin
  a.balance
rescue NoMethodError
  puts "extern getter denied"
end

# --- send still bypasses (orthogonal to self-receiver) ---
puts a.send(:balance)                           # 100
a.send(:balance=, 200)
puts a.via_self_getter                          # 200

# --- Inheritance: subclass calls inherited private via self ---
class Sub < Acc
  def sub_set(v)
    self.balance = v
  end
end
s = Sub.new
s.sub_set(7)
puts s.via_self_getter                          # 7
