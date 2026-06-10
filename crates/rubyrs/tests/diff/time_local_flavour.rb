# Time local-flavour + ISO 8601 surface (jekyll date-filter chain:
# `time.dup.localtime` → strftime/xmlschema). Assumes TZ=UTC (the
# rubyrs Tier-1 contract; the runner exports it).
require "time"
t = Time.at(1_780_315_200) # 2026-06-01 12:00:00 UTC
puts t.to_s
puts t.utc?
puts t.xmlschema
l = t.dup.localtime
puts l.to_s
puts l.utc?
puts l.xmlschema
u = l.utc
puts u.to_s
puts u.utc?
puts u.xmlschema
puts t.strftime("%B %-d, %Y")
