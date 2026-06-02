# ActiveSupport-lite core-ext parity gate (ADR 0026 menu item 3).
# The SAME require path resolves to rubyrs's vendored canon
# (src/stdlib_vendor/active_support_lite.rb) and, on the CRuby oracle,
# to the real ActiveSupport gem (RubyGems enabled — run_diff_gem).
# Pinned upstream: ActiveSupport 8.0.x. Covers the whole blessed
# surface, not just the methods this branch added, so the gate also
# guards the pre-existing canon against drift from the real gem.

require "active_support/all"

# ---- blank? / present? / presence ----
puts "== blank? =="
[nil, false, true, "", "   ", "\t\n", " x ", "x", [], [1], {}, {a: 1},
 0, 1, -3, 3.5, :sym].each { |v| puts "#{v.inspect}.blank? => #{v.blank?}" }

puts "== present? / presence =="
[nil, false, "", "  ", "x", [], [1], {}, 0].each { |v| puts "#{v.inspect} => present?=#{v.present?} presence=#{v.presence.inspect}" }

# ---- Object#try / try! / in? ----
puts "== try / in? =="
puts nil.try(:upcase).inspect
puts "abc".try(:upcase).inspect
puts "abc".try(:nonexistent).inspect
puts nil.try!(:upcase).inspect
puts 3.in?([1, 2, 3])
puts 9.in?([1, 2, 3])
puts "b".in?(%w[a b c])

# ---- Array access + in_groups_of ----
puts "== array access =="
arr = [10, 20, 30, 40, 50]
puts [arr.second, arr.third, arr.fourth, arr.fifth].inspect
puts [1].second.inspect
puts (1..7).to_a.in_groups_of(3).inspect
puts (1..7).to_a.in_groups_of(3, 0).inspect

# ---- Hash key transforms ----
puts "== symbolize / stringify =="
h = {"a" => 1, "b" => {"c" => 2}, 3 => "untouched"}
puts h.symbolize_keys.inspect
puts h.stringify_keys.inspect
puts({a: 1, b: 2}.stringify_keys.inspect)
puts h.inspect  # non-bang leaves original intact

puts "== bang key transforms =="
h2 = {"x" => 1, "y" => 2}; h2.symbolize_keys!; puts h2.inspect
h3 = {a: 1, b: 2}; h3.stringify_keys!; puts h3.inspect

puts "== deep symbolize / stringify (incl. nested arrays) =="
nested = {"a" => 1, "b" => {"c" => {"d" => 2}}, "list" => [{"e" => 3}, [{"f" => 4}]]}
puts nested.deep_symbolize_keys.inspect
puts({a: 1, b: {c: [{d: 2}]}}.deep_stringify_keys.inspect)
n2 = {"a" => {"b" => 1}}; n2.deep_symbolize_keys!; puts n2.inspect

puts "== deep_transform_keys =="
puts nested.deep_transform_keys { |k| k.to_s.upcase }.inspect

# ---- deep_merge ----
puts "== deep_merge =="
a = {a: 1, b: {c: 2, d: 3}, e: [1, 2]}
b = {b: {c: 20, f: 4}, g: 5}
puts a.deep_merge(b).inspect
puts a.inspect  # non-bang leaves original intact
puts a.deep_merge(b) { |key, old, new| old + new }.inspect
c = {a: {x: 1}}; c.deep_merge!({a: {y: 2}, b: 3}); puts c.inspect

# ---- deep_dup ----
puts "== deep_dup =="
orig = {a: [1, 2], b: {c: 3}}
copy = orig.deep_dup; copy[:a] << 99; copy[:b][:c] = 300
puts orig.inspect
puts copy.inspect
puts [[1, 2], {k: "v"}].deep_dup.inspect
puts 5.deep_dup.inspect
puts "str".deep_dup.inspect

# ---- String inflections / utility ----
puts "== string inflections =="
%w[active_record foo_bar_baz].each { |s| puts "#{s}.camelize => #{s.camelize}" }
puts "active_record".camelize(:lower)
%w[ActiveRecord HTTPRequest FooBarBaz].each { |s| puts "#{s}.underscore => #{s.underscore}" }
puts "puma_server".dasherize
puts "puni_puni".titleize
puts "employee_id".humanize
puts "hello   world\t\n  again".squish.inspect
puts "the quick brown fox".truncate(12).inspect
puts "short".truncate(20).inspect
puts "the quick brown fox".truncate(12, omission: "…").inspect
