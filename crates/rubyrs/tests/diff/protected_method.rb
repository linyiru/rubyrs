# `protected` method enforcement. A protected method can be
# called with an explicit receiver only when the caller's
# `self` is an instance of the receiver's class (or a
# descendant). External callers raise NoMethodError.

class Account
  def initialize(b); @b = b; end

  # `>` calls `other.balance` — both self and other are
  # Account, so the protected access is allowed.
  def >(other)
    balance > other.balance
  end

  protected
  def balance
    @b
  end
end

a = Account.new(100)
b = Account.new(50)
puts a > b                                # true (same-class access)
puts a.balance rescue puts "ok: outside"  # NoMethodError from main scope

# Subclass instance is still "same family" — protected access allowed.
class SavingsAccount < Account
end

c = SavingsAccount.new(75)
puts a > c                                # true (subclass still kind_of?(Account))
puts c > a                                # false (75 > 100 is false)

# Implicit self.balance inside another method — always allowed.
class WithSelf
  def initialize(v); @v = v; end
  def doubled
    raw * 2                                # bare call, self.raw equivalent
  end
  protected
  def raw
    @v
  end
end
puts WithSelf.new(7).doubled              # 14

# Calling a protected method on an UNRELATED class fails.
class Other
  def initialize(s); @s = s; end
  def peek_at(account)
    account.balance                       # main-scope: not same family
  end
end

begin
  Other.new("x").peek_at(a)
rescue NoMethodError
  puts "ok: cross-class denied"
end
