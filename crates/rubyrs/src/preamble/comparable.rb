# Comparable — Ruby mixin that gives a class the full comparison
# surface (`<`, `<=`, `>`, `>=`, `==`, `between?`, `clamp`) once
# the class defines `<=>`. CRuby's Comparable lives in C
# (compar.c); rubyrs's subset doesn't yet model real Module
# inheritance, so we lean on a stub class with the methods
# defined directly. Built-in numerics / Strings / Symbols /
# Times don't `include Comparable` here — their per-type fast
# paths in `vm/numeric.rs` / `vm/string.rs` / `preamble/time.rb`
# already implement the operators directly without going through
# this mixin.
#
# Loaded after Object so `is_a?(Comparable)` / `class C <
# Comparable` resolves; loaded before Enumerable to match the
# original inline preamble order.
#
# `include Comparable` copies these methods into the target
# class's method table (see `do_call`'s include-intercept).
# User-defined methods on the including class take precedence —
# the copy is non-destructive.
#
# On `<=>` returning nil (incomparable pair), the four ordered
# predicates raise ArgumentError, matching CRuby. `==` returns
# `false` instead of raising — CRuby's documented exception to
# the rule that Object equality must never raise.

class Comparable
  def <(other)
    c = self <=> other
    raise ArgumentError, "comparison failed" if c.nil?
    c < 0
  end
  def <=(other)
    c = self <=> other
    raise ArgumentError, "comparison failed" if c.nil?
    c <= 0
  end
  def >(other)
    c = self <=> other
    raise ArgumentError, "comparison failed" if c.nil?
    c > 0
  end
  def >=(other)
    c = self <=> other
    raise ArgumentError, "comparison failed" if c.nil?
    c >= 0
  end
  def ==(other)
    c = self <=> other
    return false if c.nil?
    c == 0
  end
  def between?(lo, hi)
    self >= lo && self <= hi
  end
  def clamp(*args)
    ## Range form (one arg): `clamp(lo..hi)`. Endpoints may be
    ## nil for one-sided ranges (`(..max)` / `(min..)`); a nil
    ## bound is treated as "no limit on that side", matching
    ## CRuby.
    if args.length == 1 && args[0].is_a?(Range)
      r = args[0]
      lo, hi = r.begin, r.end
      if !lo.nil? && self < lo
        lo
      elsif !hi.nil? && self > hi
        hi
      else
        self
      end
    elsif args.length == 2
      lo, hi = args[0], args[1]
      if !lo.nil? && self < lo
        lo
      elsif !hi.nil? && self > hi
        hi
      else
        self
      end
    else
      raise ArgumentError, "wrong number of arguments (given #{args.length}, expected 1..2)"
    end
  end
end
