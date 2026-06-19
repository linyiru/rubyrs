# Pure-Ruby Date/DateTime (Tier 3 vendored): civil dates on a
# Julian-Day-Number core; now/today via Time.now.
require "date"
d = Date.new(2026, 6, 18)
p d.to_s
p [d.year, d.month, d.day, d.wday, d.yday]
p [d.mon, d.mday]
p d.leap?
p Date.new(2024, 2, 29).leap?
p (d + 20).to_s
p (d - 20).to_s
p (d - Date.new(2026, 1, 1))
p (d >> 1).to_s
p (d << 2).to_s
p (Date.new(2026,1,31) >> 1).to_s   # clamps to Feb 28
p d.next_day.to_s
p d.succ.to_s
p d.strftime("%Y-%m-%d %A (%a) %B")
p d.iso8601
p Date.parse("2026-06-18").to_s
p (Date.new(2026,6,18) <=> Date.new(2026,6,19))
p (Date.new(2026,6,18) == Date.new(2026,6,18))
p Date.jd(Date.new(2026,6,18).jd).to_s
dt = DateTime.new(2026, 6, 18, 12, 34, 56, "+00:00")
p dt.to_s
p [dt.hour, dt.min, dt.sec]
p dt.strftime("%H:%M:%S %p")
p DateTime.parse("2026-06-18T09:08:07+00:00").to_s
p (Date.new(2026,1,1) == DateTime.new(2026,1,1,0,0,0))
p (Date.new(2026,1,1) == DateTime.new(2026,1,1,12,0,0))
