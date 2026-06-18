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

module Comparable
  # CRuby's `rb_cmperr` message: `comparison of <self class> with
  # <other> failed`, where `<other>` is the VALUE for a Numeric / nil /
  # true / false operand (e.g. `5`) and the CLASS name otherwise (e.g.
  # `String`). So `5 < "x"` → "...Integer with String failed" but
  # `"a" < 5` → "...String with 5 failed".
  def __cmp_fail(other)
    rhs = case other
          when Numeric, nil, true, false then other.inspect
          else other.class
          end
    raise ArgumentError, "comparison of #{self.class} with #{rhs} failed"
  end
  private :__cmp_fail

  def <(other)
    c = self <=> other
    __cmp_fail(other) if c.nil?
    c < 0
  end
  def <=(other)
    c = self <=> other
    __cmp_fail(other) if c.nil?
    c <= 0
  end
  def >(other)
    c = self <=> other
    __cmp_fail(other) if c.nil?
    c > 0
  end
  def >=(other)
    c = self <=> other
    __cmp_fail(other) if c.nil?
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

## Mix `Comparable` into `Numeric` — CRuby's
## `Integer.include?(Comparable)` is true, and `between?` /
## `clamp` come from there. Numeric is defined in
## preamble/object.rb (loaded earlier), but the `include` has to
## wait until HERE because the `Comparable` constant only exists
## once the block above has run. The primitive comparison ops
## (`Int < Int`, etc.) are intercepted by `primitive_call`
## BEFORE the method-table walk, so the fast path is unaffected;
## Comparable supplies only the non-primitive `between?` /
## `clamp` (Integer/Float/Rational inherit via `< Numeric`).
class Numeric
  include Comparable
end

## Mix `Comparable` into `String` too — CRuby's
## `String.include?(Comparable)` is true; `between?` / `clamp`
## build on String's native `<=>`. As with Numeric, the primitive
## comparison ops (`Str < Str`, `==`, …) are intercepted by
## `primitive_call` BEFORE the method-table walk, so the native
## fast paths still win — Comparable only supplies the otherwise-
## missing `between?` / `clamp`. (`"abc" == 5` stays `false`
## rather than raising, because native `==` handles it first.)
class String
  include Comparable
end
