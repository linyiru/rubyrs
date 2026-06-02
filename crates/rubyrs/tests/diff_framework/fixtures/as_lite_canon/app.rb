# Tier-1 ActiveSupport-lite canon parity — runs `require
# "active_support"` (CRuby) / `require "active_support"` (rubyrs,
# routes to embedded canon) and exercises the Tier A+B+C surface.
# Stdout is byte-diffed cross-runtime by the diff_framework harness.
#
# Symmetric require: CRuby's `active_support/all` is what the
# manifest declares as the required gem so the gem-availability
# probe succeeds; the actual app loads `active_support` (real
# gem auto-loads core_ext on first method call). rubyrs's canon
# matches either name via stdlib_vendor_source's match arm.
require "active_support/all"

# ---- Tier A — blank? / present? / presence ----
puts "--- blank/present/presence ---"
puts nil.blank?           # true
puts "".blank?            # true
puts "  ".blank?          # true
puts "x".blank?           # false
puts [].blank?            # true
puts({}.blank?)           # true
puts 0.blank?             # false
puts false.blank?         # true
puts true.blank?          # false
puts "  ".present?        # false
puts "x".present?         # true
p nil.presence            # nil
p "".presence             # nil
p "x".presence            # "x"

# ---- Tier A — Array extras ----
puts "--- array extras ---"
p [1, 2, 3, 4, 5].second
p [1, 2, 3, 4, 5].third
p [1, 2, 3, 4, 5].fourth
p [1, 2, 3, 4, 5].fifth
p [1, 2, 3, 4, 5, 6, 7].in_groups_of(3)
p [1, 2, 3, 4, 5, 6, 7].in_groups_of(3, false)
p [1, 2, 3, 4, 5, 6, 7].in_groups_of(3, "x")
p [].blank?

# ---- Tier A — Object#try / Object#in? ----
puts "--- try / in? ---"
p nil.try(:upcase)
p "hi".try(:upcase)
p "hi".try(:not_a_method)
p 2.in?([1, 2, 3])
p 99.in?([1, 2, 3])
p "a".in?(%w[a b c])

# ---- Tier C — Hash transforms ----
puts "--- hash transforms ---"
p({a: 1, b: 2}.symbolize_keys)        # already symbol, no-op
p({"a" => 1, "b" => 2}.symbolize_keys)
p({a: 1, b: 2}.stringify_keys)
p({"a" => 1, "b" => 2}.stringify_keys) # already string, no-op
p({a: {b: {c: 1}}, d: 2}.deep_stringify_keys)
p({"a" => {"b" => 1}, "c" => 2}.deep_symbolize_keys)
# deep with mixed Array of Hashes
p({list: [{a: 1}, {b: 2}]}.deep_stringify_keys)
p({a: 1, b: {c: 2}}.deep_merge({a: 9, b: {d: 3}}))
# deep_merge with deeper conflict — h1[:b][:c] vs h2[:b][:c] — h2 wins on non-Hash
p({a: {b: {c: 1, d: 4}}}.deep_merge({a: {b: {c: 9, e: 5}}}))

# ---- Tier B — String slice ----
puts "--- string slice ---"
puts "  hello   world  ".squish
puts "active_record".camelize
puts "active_record".camelize(:lower)
puts "active_record_base".camelize
puts "ActiveRecord".underscore
puts "ActiveRecordBase".underscore
puts "HTTPRequest".underscore
puts "puma_server".dasherize
puts "puni puni".titleize
puts "active_record".titleize
puts "employee_id".humanize
puts "first_name".humanize
puts "Once upon a time in a far far away".truncate(20)
puts "short".truncate(20)
puts "exactly twenty char.".truncate(20)
