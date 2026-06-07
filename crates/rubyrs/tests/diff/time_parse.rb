# Time.parse for the ISO-8601-ish front-matter / filename-date subset.
# strftime only on +0000/Z inputs (display is zone-sensitive and rubyrs
# is UTC-only); offsets are checked via #to_i (absolute epoch, which is
# zone-independent so it matches CRuby regardless of the host TZ).
require "time"
p Time.parse("2026-06-06 12:30:45 +0000").strftime("%Y-%m-%d %H:%M:%S")
p Time.parse("2024-12-31T23:59:00Z").strftime("%Y/%m/%d %H:%M")
p Time.parse("2026-06-06").strftime("%Y/%m/%d")
p Time.parse("2000-01-01 00:00:00 +0000").to_i
p Time.parse("2026-06-06 09:00:00 +0900").to_i
p Time.parse("2026-06-06 09:00:00 -0500").to_i
p Time.parse("2026-06-06 12:00:00 +0000").year
p [Time.parse("2026-06-06 +0000").month, Time.parse("2026-06-06 +0000").day]
