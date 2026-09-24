# Time.at(sec, subsec, unit) — the third argument names subsec's
# unit (default :usec).
p Time.at(1, 250).nsec
p Time.at(1, 250, :usec).nsec
p Time.at(1, 250, :microsecond).nsec
t = Time.at(0, 1_500, :millisecond)
p [t.to_i, t.nsec]
p Time.at(0, 123_456_789, :nanosecond).nsec
p Time.at(0, 7, :nsec).nsec
t = Time.at(5, 2_000_000_001, :nsec)
p [t.to_i, t.nsec]
p Time.at(1.5, 1, :millisecond).nsec

begin
  Time.at(0, 1, :bogus)
rescue ArgumentError => e
  p e.class
end
