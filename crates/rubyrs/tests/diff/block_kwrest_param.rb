# Block / proc / lambda `|**opts|` keyword-rest parameter binds
# the trailing keyword arguments as a Hash (`{}` when none),
# matching CRuby. Pre-fix `**opts` bound `nil` (the param was
# dropped at AST translation — BlockParam had no KwRest variant).
#
# Discovery: P3 Sinatra spike discovery-map (cluster item 1 —
# "THE hard wall in seg1": mustermann threads `**opts` blocks).
#
# Skipped under STRESS_GC: the kw-rest binding itself is GC-safe
# (a 100x splat+kwrest loop passes under STRESS_GC=1), but this
# fixture's long sequence of proc/lambda allocations cumulatively
# trips the SAME pre-existing block-closure GC root-hole the
# sibling forwardable_shim.rb / struct_factory.rb fixtures
# document (captured slots swept mid-dispatch under heap
# pressure). Normal-mode binding — the contract — is correct.
# Sentinel-skip (not `exit 0`, which prints "exit (SystemExit)").

if ENV["STRESS_GC"]
  # Empty body — both runtimes emit nothing.
else

# proc / lambda with bare **o.
puts "proc_empty=#{proc { |**o| o }.call.inspect}"
puts "proc_filled=#{proc { |**o| o }.call(x: 1, y: 2).inspect}"
puts "lambda_empty=#{lambda { |**o| o }.call.inspect}"
puts "lambda_filled=#{->(**o) { o }.call(a: 9).inspect}"

# Block passed to an iterator (real invoke_block path).
puts "map_empty=#{[1].map { |**o| o }.inspect}"
puts "each_count=#{[1, 2, 3].map { |**o| o.size }.inspect}"

# Mixed: leading positional + **o.
puts "pos_kw=#{proc { |a, **o| [a, o] }.call(1, x: 2).inspect}"
puts "pos_kw_nokw=#{proc { |a, **o| [a, o] }.call(5).inspect}"

# Splat + **o.
puts "splat_kw=#{proc { |*a, **o| [a, o] }.call(1, 2, k: 3).inspect}"
puts "splat_kw_nokw=#{proc { |*a, **o| [a, o] }.call(1, 2).inspect}"

# NB: define_method'd blocks with `**o` (installed AS a method,
# dispatched via invoke_method rather than invoke_block) are a
# SEPARATE path not covered here — that binder needs its own
# kw-rest slot fix. The iterator/proc/lambda block path (the one
# mustermann's `**opts` blocks hit) is what this defends.

# Anonymous **  (reserve, drop — no name to read, but no crash).
puts "anon=#{proc { |**| 42 }.call(ignored: 1)}"

# NB: an EXPLICIT positional Hash `call({z: 9})` (vs kwargs
# `call(z: 9)`) is NOT asserted — CRuby 3.x binds `**o` to `{}`
# there (the positional hash is dropped), but invoke_block can't
# see the kwargs-vs-positional distinction (no kwargs_trailing
# signal reaches it), so rubyrs peels it as kwargs. Documented
# edge divergence; the kwargs form (the common case) is correct.

# Non-kwrest block with a trailing hash arg keeps it positional
# (regression: don't peel when there's no **o).
puts "no_kwrest=#{proc { |a| a }.call({m: 1}).inspect}"

end
