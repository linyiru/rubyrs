# DateTime method-surface parity (vendored stdlib_vendor/date.rb).
# Covers the gaps filled alongside Date: time-preserving +/- (Date's
# would drop the time-of-day), the offset views (offset/zone/
# sec_fraction/new_offset), the iso8601/strptime class parsers, and
# the CRuby inspect tuple (UTC jd/seconds + offset). Runs under
# --features stdlib with CRuby's core `date` as the oracle.
require "date"

dt = DateTime.new(2026, 6, 23, 14, 30, 45)
dtz = DateTime.new(2026, 6, 23, 14, 30, 45, "+09:00")

# Accessors.
puts [dt.year, dt.month, dt.day, dt.hour, dt.min, dt.sec].inspect
p dt.offset
p dtz.offset
p dtz.zone
p dt.sec_fraction

# Time-preserving arithmetic.
p (dt + 1).to_s
p (dt - 1).to_s
p (dt + Rational(1, 2)).to_s
p (dt - DateTime.new(2026, 6, 22, 14, 30, 45))
p (dt - DateTime.new(2026, 6, 23, 2, 30, 45))

# Offset re-expression (same instant).
p dtz.new_offset("+00:00").to_s
p dt.new_offset("+09:00").to_s
p DateTime.new(2026, 6, 23, 2, 0, 0, "+09:00").new_offset("+00:00").to_s

# Class parsers.
p DateTime.parse("2026-06-23T14:30:45+09:00").to_s
p DateTime.iso8601("2026-06-23T14:30:45+00:00").to_s
p DateTime.strptime("2026-06-23 14:30", "%Y-%m-%d %H:%M").to_s
p DateTime.strptime("2026-06-23 14:30:45", "%Y-%m-%d %H:%M:%S").to_s

# Display (inspect tuple uses the UTC instant).
p dt.to_s
p dt.iso8601
p dt.inspect
p dtz.inspect
p DateTime.new(2026, 6, 23, 2, 0, 0, "+09:00").inspect
p dt.strftime("%FT%T%:z")
