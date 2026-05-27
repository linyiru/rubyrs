# `Class#<` / `<=` / `>` / `>=` — subclass relation operators.
# CRuby semantics:
#   A <  B → true if A is a strict descendant of B
#   A <= B → A == B OR A < B
#   A >  B → reversed
#   A >= B → reversed
# Unrelated classes return NIL (not false). Non-Class/Module
# arg raises TypeError.
#
# Surfaced via PR #240's fixture work; the operators were
# missing entirely (NoMethodError). Used pervasively by user
# code for "is this a subclass of X?" tests, and by some
# Comparable-style mixins.

class A; end
class B < A; end
class C; end
module M; end
class D; include M; end

# --- Strict subclass ---
puts (B < A).inspect       # true
puts (A < B).inspect       # false (reversed direction)
puts (B < B).inspect       # false (not strict)
puts (B < C).inspect       # nil (unrelated)

# --- Subclass-or-equal ---
puts (B <= A).inspect      # true
puts (A <= B).inspect      # false
puts (B <= B).inspect      # true (equal)
puts (B <= C).inspect      # nil

# --- Reversed (superclass / superclass-or-equal) ---
puts (A > B).inspect       # true
puts (B > A).inspect       # false
puts (B > B).inspect       # false
puts (B > C).inspect       # nil

puts (A >= B).inspect      # true
puts (B >= A).inspect      # false
puts (B >= B).inspect      # true
puts (B >= C).inspect      # nil

# --- Inclusion in a module counts as descent ---
puts (D < M).inspect       # true
puts (D <= M).inspect      # true
puts (M > D).inspect       # true (reversed)
puts (M >= D).inspect      # true

# --- Transitive through superclass chain ---
# B < A; A defaults to Object (post-PR-#240). So:
puts (B < Object).inspect  # true (B → A → Object)
puts (B <= Object).inspect # true

# --- respond_to? whitelist parity ---
puts A.respond_to?(:<)        # true
puts A.respond_to?(:<=)       # true
puts A.respond_to?(:>)        # true
puts A.respond_to?(:>=)       # true

# --- TypeError on non-Class/Module arg ---
begin
  B < "string"
  puts "no raise (BAD)"
rescue TypeError => e
  puts "TypeError"
end
begin
  B <= 42
  puts "no raise (BAD)"
rescue TypeError => e
  puts "TypeError"
end

# --- Wrong-arity raises ArgumentError ---
begin
  A.send(:<)
  puts "no raise (BAD)"
rescue ArgumentError
  puts "ArgumentError (0 args)"
end
begin
  A.send(:<, A, A)
  puts "no raise (BAD)"
rescue ArgumentError
  puts "ArgumentError (2 args)"
end
