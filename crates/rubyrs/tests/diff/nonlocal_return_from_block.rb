# Non-local `return` from inside an Enumerable block must escape
# the enclosing method, NOT raise NoMethodError on the enumerator.
#
# Background: a family of iter.rs drivers had
#   if method_return.is_some() { return Ok(None); }
# inside their per-element loop. Returning `Ok(None)` from
# `collection_call_block` tells the dispatcher "no primitive
# matched this call" — which then falls through to looking for
# another handler and ends at NoMethodError, even though the
# block's `return` already set `method_return` and the outer
# dispatch loop was ready to unwind. The fix returns
# `Ok(Some(Value::Nil))` instead so the call is treated as
# matched, and the dispatcher's `method_return` check unwinds
# correctly. Surfaced by Copilot review on #166; same shape as
# the pre-existing fix on Array#sort/#sort! (vm/iter.rs's
# block-form sort comparator arm).
#
# This fixture pins one example per affected driver so a future
# regression bisects to the driver that lost the fix.

def via_filter_map
  [1, 2, 3].filter_map { return :via_filter_map }
  :unreached_fm
end
puts via_filter_map                                  # via_filter_map

def via_hash_filter_map
  { a: 1, b: 2 }.filter_map { return :via_hash_fm }
  :unreached_hfm
end
puts via_hash_filter_map                             # via_hash_fm

def via_transform_keys
  { a: 1, b: 2 }.transform_keys { return :via_tk }
  :unreached_tk
end
puts via_transform_keys                              # via_tk

def via_transform_values
  { a: 1, b: 2 }.transform_values { return :via_tv }
  :unreached_tv
end
puts via_transform_values                            # via_tv

def via_step
  (1..3).step(1) { return :via_step }
  :unreached_step
end
puts via_step                                        # via_step

def via_take_while
  [1, 2, 3].take_while { return :via_take }
  :unreached_take
end
puts via_take_while                                  # via_take

def via_drop_while
  [1, 2, 3].drop_while { return :via_drop }
  :unreached_drop
end
puts via_drop_while                                  # via_drop

# chunk_while: bare-form is rubyrs-eager / CRuby-lazy (Enumerator),
# so `[1,2,3].chunk_while { return :x }` diverges. The `.to_a`
# form sidesteps that pre-existing divergence — both runtimes
# materialise the result, both invoke the block at least once,
# both see the non-local return escape to `def via_chunk_while`.
def via_chunk_while
  [1, 2, 3].chunk_while { return :via_cw }.to_a
  :unreached_cw
end
puts via_chunk_while                                 # via_cw

def via_min_by_n
  [1, 2, 3].min_by(2) { return :via_minby }
  :unreached_min
end
puts via_min_by_n                                    # via_minby

def via_max_by_n
  [1, 2, 3].max_by(2) { return :via_maxby }
  :unreached_max
end
puts via_max_by_n                                    # via_maxby

# Five additional sites surfaced by /code-review on #166 — same
# `Ok(None)` on method_return pattern, different methods. Pre-fix
# all five raised NoMethodError; post-fix they propagate
# `method_return` via `Ok(Some(Value::Nil))`.

def via_gsub
  "abc".gsub(/./) { return :via_gsub }
  :unreached_gsub
end
puts via_gsub                                        # via_gsub

def via_sub
  "abc".sub(/./) { return :via_sub }
  :unreached_sub
end
puts via_sub                                         # via_sub

def via_scan_caps
  "abcabc".scan(/(a)/) { return :via_scan_caps }
  :unreached_caps
end
puts via_scan_caps                                   # via_scan_caps

def via_scan_no_caps
  "abcabc".scan(/a/) { return :via_scan_no_caps }
  :unreached_no_caps
end
puts via_scan_no_caps                                # via_scan_no_caps

def via_scan_str
  "abcabc".scan("a") { return :via_scan_str }
  :unreached_scan_str
end
puts via_scan_str                                    # via_scan_str

def via_bsearch
  [1, 2, 3, 4].bsearch { return :via_bsearch }
  :unreached_bs
end
puts via_bsearch                                     # via_bsearch
