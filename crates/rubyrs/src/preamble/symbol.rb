# `Symbol#to_proc` — `:upcase.to_proc.call("hi") == "HI"`. The block
# captures the symbol and forwards every call to `recv.send(sym, *rest,
# &blk)`, so it works for receivers of any arity (`:+.to_proc.call(2, 3)`,
# `arr.reduce(&:+.to_proc)`) and explicit conversions
# (`[1, 2].map(&:to_s.to_proc)`). CRuby returns a LAMBDA, so this uses
# `lambda` to match `:sym.to_proc.lambda? == true` (rubyrs doesn't yet
# enforce lambda strict-arity, so it behaves the same for dispatch).
#
# (The literal `&:sym` block-pass has its own native fast path; this
# method covers the EXPLICIT `:sym.to_proc` calls that path doesn't.)
class Symbol
  def to_proc
    sym = self
    lambda { |recv, *args, &blk| recv.send(sym, *args, &blk) }
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

  # `Symbol#[]` — `to_s[...]` in CRuby (index/range/regex/substring slicing of
  # the symbol's string). Surfaced by ostruct's method_missing, which extracts
  # a setter name with `mid[/.*(?==\z)/m]`.
  def [](*args)
    to_s[*args]
  end

  # `Symbol#start_with?` / `#end_with?` — delegate to the string form
  # (CRuby Symbol exposes both). dry-configurable's config method_missing
  # peels a `name=` setter with `name.end_with?("=")`.
  def start_with?(*args)
    to_s.start_with?(*args)
  end

  def end_with?(*args)
    to_s.end_with?(*args)
  end
end
