# ActiveSupport-lite Tier D-narrow parity gate. Same source on
# both sides: rubyrs resolves `require "active_support/all"` to
# the vendored canon (src/stdlib_vendor/active_support_lite.rb)
# under `--features stdlib`; the CRuby oracle (run_diff_gem
# harness) loads the real ActiveSupport 8.0.x gem. The
# byte-diff covers Duration construction, arithmetic, inspect,
# and Time integration — every shape `poc/as_lite/GAPS.md`
# §Tier D originally listed.
#
# What's intentionally absent from this fixture: month/year
# durations (`1.month`, `2.years`, `1.month.from_now`) — calendar-
# correct advance is out of Tier-1 scope per the doc-block at
# the top of active_support_lite.rb. Tier D-narrow trades that
# for a self-contained pure-Ruby implementation with no tzinfo
# dependency.
require "active_support/all"

# ---- Numeric helpers: singular + plural produce identical
# Duration values. Pluralisation only affects #inspect, not
# arithmetic. ----
puts 1.second.to_i
puts 1.seconds.to_i
puts 1.minute.to_i
puts 1.minutes.to_i
puts 1.hour.to_i
puts 1.hours.to_i
puts 1.day.to_i
puts 1.days.to_i
puts 1.week.to_i
puts 1.weeks.to_i
puts 1.fortnight.to_i
puts 1.fortnights.to_i

# ---- Multi-unit arithmetic — parts preserved, NOT collapsed.
# (1.week + 1.day) must inspect as "1 week and 1 day", not
# "8 days". 8.days.inspect stays "8 days". ----
puts (1.week + 1.day).to_i
puts (1.week + 1.day).inspect
puts 8.days.inspect

# ---- inspect canonical formatting. ----
puts 1.second.inspect
puts 2.seconds.inspect
puts 1.minute.inspect
puts 2.minutes.inspect
puts 1.hour.inspect
puts 2.hours.inspect
puts 1.day.inspect
puts 2.days.inspect
puts 1.week.inspect
puts 2.weeks.inspect
puts 2.fortnights.inspect  # → "4 weeks" (canonicalised at construction)
puts 0.seconds.inspect      # → "0 seconds" (must not be empty string)

# ---- Multi-component inspect. Two parts use " and "; three+
# use Oxford-comma form "a, b, and c". ----
puts (1.hour + 30.minutes).inspect
puts (1.day + 1.hour).inspect
puts (1.hour + 1.minute + 1.second).inspect
puts (2.days + 3.hours + 45.minutes).inspect
puts (1.week + 2.days + 3.hours + 4.minutes + 5.seconds).inspect

# ---- Signed values — singular only when value == 1. Negative
# uses plural form regardless of magnitude. ----
puts (-1).seconds.inspect       # → "-1 seconds" (plural!)
puts (-2).hours.inspect          # → "-2 hours"
puts (1.minute - 30.seconds).inspect   # → "1 minute and -30 seconds"
                                       # (NOT auto-canonicalised
                                       #  to "30 seconds")

# ---- to_i collapses to total signed seconds across all parts.
puts (1.hour - 30.minutes).to_i  # → 1800
puts (1.day + (-1).hours).to_i   # → 82800
puts (1.fortnight - 1.day).to_i  # → 1123200

# ---- Multiplication scales every part.
puts (1.day * 7).to_i             # → 604800
puts (1.day * 7).inspect          # → "7 days"
puts (30.minutes * 2).inspect     # → "60 minutes"

# ---- Negation flips every part's sign.
puts (-1.hour).inspect
puts (-(1.day + 30.minutes)).inspect

# ---- Time integration. Use a fixed Time anchor to avoid
# wall-clock flakiness. ----
anchor = Time.at(1_700_000_000)
puts (anchor + 1.day).to_i        # anchor + 86400
puts (anchor - 1.day).to_i        # anchor - 86400
puts (anchor + (1.hour + 30.minutes)).to_i

# ago/since with an explicit `now` arg — same anchor.
puts 1.hour.ago(anchor).to_i      # anchor - 3600
puts 1.hour.since(anchor).to_i    # anchor + 3600
puts 1.hour.from_now(anchor).to_i # alias for since
puts 1.hour.until(anchor).to_i    # alias for ago

# Cross-Duration arithmetic.
combined = 1.hour + 30.minutes
puts combined.to_i
puts combined.inspect

# Class check — both runtimes name the class
# ActiveSupport::Duration.
puts 1.hour.class.name
puts 1.day.from_now(anchor).class.name
