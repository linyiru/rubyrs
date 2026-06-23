# Date method-surface parity (vendored stdlib_vendor/date.rb). Covers
# the gaps filled alongside the numeric sweep: commercial date
# (cwday/cweek/cwyear), instance iteration (step/upto/downto), the
# lilian/modified-julian accessors (ld/mjd), the CRuby inspect tuple
# format, and Date.strptime. Runs under --features stdlib with CRuby's
# real `date` as the oracle (run_diff_gem).
require "date"

d = Date.new(2026, 6, 23)

# Core accessors / predicates (regression guard for the existing impl).
puts [d.year, d.month, d.day, d.wday, d.yday, d.leap?].inspect
puts [d.jd, d.ld, d.mjd].inspect

# Commercial (ISO-8601) date across year boundaries.
[[2026, 1, 1], [2025, 12, 29], [2021, 1, 1], [2020, 12, 31], [2016, 1, 1]].each do |y, m, dd|
  c = Date.new(y, m, dd)
  puts "#{c} cwday=#{c.cwday} cweek=#{c.cweek} cwyear=#{c.cwyear}"
end

# Arithmetic + navigation.
puts [(d + 7).to_s, (d - 7).to_s, (d - Date.new(2026, 6, 1)).to_s].inspect
puts [(d << 1).to_s, (d >> 1).to_s, d.next_month.to_s, d.next_year.to_s].inspect

# Iteration.
p Date.new(2026, 1, 1).step(Date.new(2026, 1, 10), 3).map(&:day)
p Date.new(2026, 1, 10).downto(Date.new(2026, 1, 8)).map(&:day)
p Date.new(2026, 1, 1).upto(Date.new(2026, 1, 3)).to_a.size

# Parsing.
p Date.strptime("2026-06-23", "%Y-%m-%d").to_s
p Date.strptime("23/06/2026", "%d/%m/%Y").to_s
p Date.strptime("06/23/26", "%m/%d/%y").to_s
p Date.parse("2026-06-23").to_s

# Display.
p d.to_s
p d.iso8601
p d.inspect
p d.strftime("%Y/%m/%d %A %B")
