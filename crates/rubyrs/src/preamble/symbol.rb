# `Symbol#to_proc` — `:upcase.to_proc.call("hi") == "HI"`. The block
# captures the symbol and forwards every call to `recv.send(sym, *rest,
# &blk)`, so it works for receivers of any arity (`:+.to_proc.call(2, 3)`,
# `arr.reduce(&:+.to_proc)`) and explicit conversions
# (`[1, 2].map(&:to_s.to_proc)`). CRuby returns a lambda; rubyrs has no
# `Proc#lambda?` to distinguish, and the proc form behaves identically
# for the symbol-dispatch use, so we use `proc`.
#
# (The literal `&:sym` block-pass has its own native fast path; this
# method covers the EXPLICIT `:sym.to_proc` calls that path doesn't.)
class Symbol
  def to_proc
    sym = self
    proc { |recv, *args, &blk| recv.send(sym, *args, &blk) }
  end

  # `Symbol#match` / `#match?` are `to_s.match(...)` in CRuby — the symbol
  # matches as its string. Surfaced by ostruct/oj, which guard attribute
  # names with `name.match(/.../)`.
  def match(*args, &block)
    to_s.match(*args, &block)
  end

  def match?(*args)
    to_s.match?(*args)
  end
end
