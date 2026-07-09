# Ticket 1: universal-name overrides on a CLASS receiver / class
# singleton must win over the builtin. `nil?` / `!` / `!@` are answered
# for a Class receiver by primitive_call's UNIVERSAL arms, which fired
# BEFORE the canonical class-singleton dispatch — a `def Bar.nil?` /
# `def self.!` singleton override was silently shadowed (unlike is_a?
# / === / respond_to? / to_s, whose singleton overrides already won).

class Bar; end
def Bar.nil?; "X"; end
5.times { Bar.nil? }        # warm the site
p Bar.nil?                  # => "X" (not false)

class Baz; def self.nil?; "NIL-SELF"; end; end
p Baz.nil?

class Neg; def self.!; "NOT-OVERRIDE"; end; end
p((!Neg))

# Regression guard: the already-working singleton forms stay correct.
class Q; end
def Q.is_a?(x); "IA"; end
p Q.is_a?(Object)
def Q.===(o); "EQ:#{o}"; end
p(Q === 5)
def Q.respond_to?(m, *a); "RT"; end
p Q.respond_to?(:foo)
def Q.to_s; "TS"; end
p Q.to_s

# Negative: a Class with NO override keeps the builtin answer.
class Plain; end
p Plain.nil?                # => false
p((!Plain))                 # => false
p Plain.is_a?(Object)       # => true (Class is_a Object)
