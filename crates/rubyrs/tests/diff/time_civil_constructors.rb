# Time.utc / .gm / .local / .mktime — civil-field constructors.
# Tier-1 is UTC-only so local == utc (diff harness pins TZ=UTC on
# CRuby, where that is also true).
p Time.utc(2042).year
t = Time.utc(2026, 6, 11, 12, 30, 5)
p [t.year, t.month, t.day, t.hour, t.min, t.sec]
p Time.gm(1999, 12, 31, 23, 59, 59).to_i
p Time.local(2042, 1, 1).year
p Time.mktime(1970, 1, 1).to_i
p (Time.local(2042, 1, 1) > Time.utc(2026, 1, 1))
