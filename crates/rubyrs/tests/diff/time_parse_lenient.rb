# Time.parse scans for an embedded timestamp, ignoring surrounding text
# (CRuby Date._parse leniency). Sinatra's time_for does
# Time.parse(value.to_s) on arbitrary objects (e.g. a Struct's #to_s).
require "time"
a = Time.parse("2026-06-18 22:45:32 +0000")
b = Time.parse("#<struct to_time=2026-06-18 22:45:32 +0000>")
p a.to_i == b.to_i
p Time.parse("prefix 2020-01-02 suffix").year
p Time.parse("2026-06-18T12:34:56+00:00").to_i == Time.parse("junk 2026-06-18T12:34:56+00:00 junk").to_i
p Time.parse("2026-06-18").year
begin; Time.parse("no date here at all"); rescue ArgumentError => e; p e.class; end
