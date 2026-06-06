# Empty / nil keyword-splat contributes ZERO arguments, matching
# CRuby: `f(**{})` and `f(**nil)` pass nothing. Pre-fix rubyrs
# added the empty hash (or nil) as a phantom trailing positional,
# so `pos(**{})` was `[{}]` (CRuby `[]`) and `pos(**nil)` was
# `[nil]`. A non-empty kwsplat is still passed through.
#
# Discovery: P3 Sinatra spike discovery-map cluster — mustermann's
# AST compiler threads `**opts` that are frequently empty/nil.

def pos(*a); a end

# Empty / nil kwsplat -> nothing.
puts "empty=#{pos(**{}).inspect}"
h = {}
puts "empty_var=#{pos(**h).inspect}"
puts "nil=#{pos(**nil).inspect}"

# Non-empty kwsplat -> the hash reaches a *rest callee positionally.
puts "nonempty=#{pos(**{x: 1}).inspect}"

# Mixed: leading positionals + empty/nil/filled kwsplat.
puts "mix_empty=#{pos(1, 2, **{}).inspect}"
puts "mix_nil=#{pos(1, **nil).inspect}"
puts "mix_filled=#{pos(1, **{k: 9}).inspect}"

# A **k callee: empty/nil splat -> {} bound; filled -> the hash.
def krest(**k); k end
puts "krest_empty=#{krest(**{}).inspect}"
puts "krest_nil=#{krest(**nil).inspect}"
puts "krest_filled=#{krest(a: 1, b: 2).inspect}"

# Real keyword params unaffected; kwsplat into them works.
def kw(a:, b: 9); [a, b] end
puts "kw_lit=#{kw(a: 1, b: 2).inspect}"
puts "kw_splat=#{kw(**{a: 5}).inspect}"
puts "kw_default=#{kw(a: 1).inspect}"

# Round kwarg dispatch still works (and empty splat degrades).
puts "round_kw=#{2.5.round(half: :even)}"
puts "round_empty=#{3.14159.round(**{})}"

# An EXPLICIT positional empty hash (not kwsplat) is still passed.
def one(x); x end
puts "pos_hash=#{one({}).inspect}"

# Empty splat to a method missing a required kw -> ArgumentError.
begin
  kw(**{})
rescue ArgumentError
  puts "missing_kw=ok"
end
