# `x += 1` / `@x += 1` past the Integer limit and with a Ruby-defined
# `+` (#418): the fast paths bail at i64::MAX so `+` promotes to Bignum,
# and the slow path runs a Ruby `+` to completion before reading it.
# Every form promotes: arena, own cell, captured cell, expression
# value, ivars, and a hot loop that crosses the limit after the JITs
# have compiled it.
M = 2**63 - 1
def inc_plain; a = M; a += 1; a; end
def inc_blk; a = M; [1].each { }; a += 1; a; end
def inc_inblk; r = nil; [1].each { a = M; a += 1; r = a }; r; end
def inc_cap; a = M; [1].each { a += 1 }; a; end
def inc_expr; a = M; b = (a += 1); [a, b]; end
p [inc_plain, inc_blk, inc_inblk, inc_cap, inc_expr]
class IvInc
  def initialize; @n = M; end
  def bump; @n += 1; end
  def bump_stmt; @n += 1; nil; end
  attr_reader :n
end
o = IvInc.new; p o.bump
o = IvInc.new; o.bump_stmt; p o.n
def cross(start, n); i = start; k = 0; while k < n; i += 1; k += 1; end; i; end
p (1..40).map { cross(M - 20, 30) }.uniq
def cross_blk(start, n); i = start; k = 0; while k < n; i += 1; k += 1; end; [1].each { }; i; end
p (1..40).map { cross_blk(M - 20, 30) }.uniq

# `x += 1` where `+` is a Ruby method runs it to completion (#418).
class Cnt
  attr_reader :n
  def initialize(n) = @n = n
  def +(o) = Cnt.new(@n + o)
end
def uinc(x) = (i = x; i += 1; i += 1; i.n)
def uinc_expr(x) = (i = x; j = (i += 1); [i.n, j.n])
def uinc_blk(x) = (i = x; [1].each { i += 1 }; i += 1; i.n)
p [uinc(Cnt.new(0)), uinc_expr(Cnt.new(5)), uinc_blk(Cnt.new(10))]
class IvCnt
  def initialize = @c = Cnt.new(0)
  def bump = (@c += 1).n
  def bump_stmt = (@c += 1; @c.n)
end
iv = IvCnt.new; p [iv.bump, iv.bump_stmt]
