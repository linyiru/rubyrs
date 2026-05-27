# Companion to `block_break_value.rb` (PR #178) — locks in the
# break / non-local-return semantics for the four sites migrated
# to `step_block` in PR #187 (the iter.rs finishing batch).
#
# Each migrated site preserved its pre-migration handling verbatim,
# but the prior PRs in this series (#166, #178) repeatedly surfaced
# silent bugs where the pre-migration `invoke_block + pop + check`
# sequence dropped break values or routed `return` to NoMethodError.
# A diff_cruby fixture is the only thing that catches that family
# of bugs cheaply, so back-fill the four sites' break/return paths
# explicitly.
#
# Sites covered (line numbers from pre-migration iter.rs):
#   330  String#gsub / #sub  regex+block
#   677  Array#chunk
#  1466  Array#bsearch
#  1726  Array#sort  /  #sort!
#
# Note: `Array#chunk { break }` is intentionally NOT covered —
# CRuby's `chunk` returns a lazy Enumerator and the block only
# runs during `to_a`, by which time the syntactic enclosing scope
# has already returned, so `break` raises `LocalJumpError`. rubyrs
# implements `chunk` eagerly; this is a documented divergence and
# diff_cruby can't lock in the eager-break behaviour without
# regressing CRuby parity in the other direction.

# --- String#gsub / #sub break value ---------------------------
# `gsub { break val }` returns val (NOT the partially-built
# string). Already covered by regex_sub.rb but re-asserted here
# alongside `sub` for symmetry. Receiver String, regex pattern.
puts "abcabc".gsub(/./) { |c| break :gsub_stop if c == "b"; c.upcase }  # gsub_stop
puts "abcabc".sub(/./)  { |c| break :sub_stop }                          # sub_stop

# --- Array#chunk non-local return ------------------------------
# `chunk { return val }` unwinds the caller method (rubyrs
# implements chunk eagerly so the block runs synchronously; the
# `return` triggers method_return inside step_block which
# unwinds via `break` in the driver). The outer method returns
# the `return val`.
def chunk_return
  # `.to_a` forces iteration in CRuby (chunk is lazy), so the
  # block runs there and `return` unwinds chunk_return.
  # In rubyrs chunk is eager: the block runs synchronously
  # inside the `chunk` primitive itself, `return` fires before
  # `chunk` even returns, the outer dispatch unwinds the method,
  # and `.to_a` never executes. Both interpreters end up
  # printing `:chunk_ret` even though the execution order
  # differs.
  [1, 2, 3].chunk { |x| return :chunk_ret }.to_a
  :unreached
end
puts chunk_return                                                        # chunk_ret

# --- Array#bsearch break value ---------------------------------
# `bsearch { break val }` short-circuits the binary search and
# returns val. Block return type otherwise routes via Int / Bool.
puts [1, 2, 3, 4, 5, 6, 7, 8].bsearch { |_| break :bs_break }.inspect    # :bs_break

# --- Array#sort / #sort! break value ---------------------------
# Comparator break: terminates the sort early and returns the
# break value (NOT the partially-sorted array).
puts [3, 1, 4, 1, 5, 9, 2, 6].sort  { |_, _| break :sort_break }.inspect # :sort_break
puts [3, 1, 4, 1, 5, 9, 2, 6].sort! { |_, _| break :sort_bang_break }.inspect # :sort_bang_break
