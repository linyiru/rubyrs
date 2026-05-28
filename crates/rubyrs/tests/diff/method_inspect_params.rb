# Method#inspect parameter list — render with CRuby's sigil
# discipline:
#
#   required positional  →  bare name
#   optional positional  →  `name=...`  (CRuby uses literal `=...`)
#   rest                 →  `*name`     (bare `*` for `def f(*)`)
#   required keyword     →  `name:`
#   optional keyword     →  `name: ...`
#   keyword rest         →  `**name`    (bare `**` for `def f(**)`)
#   block                →  `&name`
#
# Tier-2 follow-up to PR #282 (Method#inspect format): the
# first cut joined `m.params` bare, losing the sigils CRuby's
# inspect form depends on. Now reconstructed from the Proto
# (rest_param / kw_param_defaults / kw_rest_param /
# block_param / n_required_positional).
#
# Assertions are full-string comparisons against the prefix up
# to (and including) `)>` — the ` path:line` suffix CRuby
# tacks on is dropped by truncating at the `>` after `)`.

class A
  def reqd(a, b); end
  def with_default(a, b=1); end
  def with_rest(a, *b); end
  def with_kw_req(a, k:); end
  def with_kw_opt(a, k: 5); end
  def with_kw_rest(a, **opts); end
  def with_block(&blk); end
  def kitchen_sink(a, b=1, *c, k:, kw_opt: 5, **kw, &blk); end
end

def head(m)
  s = m.inspect
  # rubyrs's inspect already ends with `)>`; CRuby appends
  # ` path:line>` after the `)`. Normalize by truncating at the
  # first `) ` (CRuby) so both forms collapse to `...)>`.
  i = s.index(") ")
  i ? (s[0, i + 1] + ">") : s
end

a = A.new
puts head(a.method(:reqd))           # #<Method: A#reqd(a, b)>
puts head(a.method(:with_default))   # #<Method: A#with_default(a, b=...)>
puts head(a.method(:with_rest))      # #<Method: A#with_rest(a, *b)>
puts head(a.method(:with_kw_req))    # #<Method: A#with_kw_req(a, k:)>
puts head(a.method(:with_kw_opt))    # #<Method: A#with_kw_opt(a, k: ...)>
puts head(a.method(:with_kw_rest))   # #<Method: A#with_kw_rest(a, **opts)>
puts head(a.method(:with_block))     # #<Method: A#with_block(&blk)>
puts head(a.method(:kitchen_sink))   # #<Method: A#kitchen_sink(a, b=..., *c, k:, kw_opt: ..., **kw, &blk)>
