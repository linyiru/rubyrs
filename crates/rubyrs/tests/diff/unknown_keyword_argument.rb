# A method that declares NAMED keyword params but NO `**kwrest`
# rejects any supplied keyword that names no declared param —
# `ArgumentError: unknown keyword: :z` (a common Rails/gem safety
# check). Previously rubyrs silently accepted the extra keyword.
# The existing missing-required-keyword error must be preserved,
# and `**kwrest` / bare-positional-Hash shapes must NOT be flagged.

def show
  yield
rescue ArgumentError => e
  puts "ArgumentError: #{e.message}"
end

def all_req(x:, y:);      "ok x=#{x} y=#{y}"; end
def mixed_opt(x:, y: 5);  "ok x=#{x} y=#{y}"; end
def with_kwrest(x:, **o); "ok x=#{x} o=#{o.inspect}"; end
def splat_and_kw(*a, x:); "ok a=#{a.inspect} x=#{x}"; end
def just_pos(h);          "ok h=#{h.inspect}"; end
def opt_only(a: 1);       "ok a=#{a}"; end

# (a) single unknown keyword → raises
show { all_req(x: 1, y: 2, z: 3) }            # unknown keyword: :z
# (b) unknown against a method with an optional kw too
show { mixed_opt(x: 1, z: 3) }                # unknown keyword: :z
# (c) TWO unknown keywords (all declared params satisfied) → plural
show { mixed_opt(x: 1, q: 8, r: 9) }          # unknown keywords: :q, :r
# (d) unknown order preserved (declared kw last)
show { mixed_opt(b: 1, a: 2, x: 3) }          # unknown keywords: :b, :a
# (e) string key via ** is unknown (inspected with quotes)
show { all_req(**{ "y" => 2, :x => 1 }) }     # unknown keyword: "y"

# missing behaviour is UNCHANGED:
# (f) single missing required
show { all_req(x: 1) }                        # missing keyword: :y
# (g) multiple missing required → plural, in declared order
show { all_req() }                            # missing keywords: :x, :y
# (h) BOTH missing and unknown → missing reported first
show { all_req(x: 1, junk: 9) }               # missing keyword: :y

# shapes that must NOT be flagged:
# (i) `**kwrest` absorbs the unknown
puts with_kwrest(x: 1, z: 3)                  # ok x=1 o={z: 3}
# (j) splat + required kw still rejects a stray keyword
show { splat_and_kw(1, 2, x: 9, z: 3) }       # unknown keyword: :z
puts splat_and_kw(1, 2, x: 9)                 # ok a=[1, 2] x=9
# (k) a method with NO kw params takes a brace/kw hash POSITIONALLY
puts just_pos(a: 1, b: 2)                     # ok h={a: 1, b: 2}
# (l) explicit brace-hash to a kw-only method is a positional (arity error,
#     NOT an unknown-keyword error — the trailing hash never becomes kwargs)
show { opt_only({ z: 3 }) }                   # wrong number of arguments (given 1, expected 0)
# (m) all declared keywords supplied → success
puts all_req(x: 10, y: 20)                    # ok x=10 y=20
puts mixed_opt(x: 10)                         # ok x=10 y=5
