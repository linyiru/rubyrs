# Tier 1 `Time` class — capability-injected wall clock + pure-Ruby
# decomposition. The fixture uses fixed epoch values fed through
# `Time.at` so the comparison is deterministic; `Time.now`-flavored
# assertions are in `tests/embed.rs` since exact `Time.now` values
# can't be diff_cruby-matched.
#
# Component / `to_s` assertions go through `.utc` explicitly so
# CRuby's local-timezone display converges to the rubyrs UTC-only
# model.
#
# Tier 1 deviations NOT exercised here (documented):
#   - `Time.at(sec, subsec, unit:)` 3-arg unit-keyword form
#     (`:millisecond` / `:nsec`); we only model the default
#     `:usec` shape for the 2-arg form.
#   - `Time.new(year, month, day, ...)` multi-arg constructor;
#     the rubyrs `Time.new(sec, nsec_raw)` shape is the
#     internal builder and tests for it live in embed.rs.
#   - `Time#strftime` — separate larger commit.

# Class identity.
puts Time.class.name           # "Class"
puts Time.at(0).class.name     # "Time"

# Epoch round-trip via Time.at.
puts Time.at(0).to_i           # 0
puts Time.at(1_700_000_000).to_i
puts Time.at(-1).to_i          # -1 (pre-epoch)

# Component decomposition (UTC-explicit so CRuby's local-tz
# doesn't surface).
t = Time.at(1_700_000_000).utc
puts t.year                    # 2023
puts t.month                   # 11
puts t.day                     # 14
puts t.hour                    # 22
puts t.min                     # 13
puts t.sec                     # 20
puts t.wday                    # 2 (Tuesday)

# Boundary epochs.
puts Time.at(0).utc.to_s                          # "1970-01-01 00:00:00 UTC"
puts Time.at(86_399).utc.to_s                     # "1970-01-01 23:59:59 UTC"
puts Time.at(86_400).utc.to_s                     # "1970-01-02 00:00:00 UTC"
puts Time.at(-86_400).utc.to_s                    # "1969-12-31 00:00:00 UTC"

# Sub-second precision via the `usec` arg (CRuby semantics).
t2 = Time.at(100, 500_000)     # 100 sec + 500_000 usec = 100.5 sec
puts t2.to_i                   # 100
puts t2.usec                   # 500_000
puts t2.nsec                   # 500_000_000
puts t2.tv_nsec                # alias of nsec
puts t2.to_f                   # 100.5

# Subsec usec normalisation — values past 1_000_000 carry into sec.
t3 = Time.at(0, 2_500_000)
puts t3.to_i                   # 2
puts t3.usec                   # 500_000

# Float seconds decompose correctly.
t4 = Time.at(1.5)
puts t4.to_i                   # 1
puts t4.nsec                   # 500_000_000

# Arithmetic — Time + Int / Time + Float return Time; Time - Time
# returns Float.
base = Time.at(1_000)
puts (base + 60).to_i          # 1060
puts (base - 60).to_i          # 940
puts (base + 1.5).to_f         # 1001.5
puts (base + 60) - base        # 60.0 (Float)
puts ((base + 60) - base).class.name  # "Float"
puts (Time.at(100) - Time.at(50)) # 50.0

# Comparison — `<=>`, `<`, `<=`, `>`, `>=`, `==` all work via
# Comparable mixin off `<=>`.
a = Time.at(100)
b = Time.at(200)
puts (a <=> b)                 # -1
puts (a == Time.at(100))       # true
puts a < b                     # true
puts b > a                     # true
puts a.between?(Time.at(50), Time.at(150)) # true
# Sub-second tiebreak via usec.
puts (Time.at(0, 1) <=> Time.at(0, 2)) # -1
puts (Time.at(0, 5) <=> Time.at(0, 5)) # 0

# `<=>` against non-Time returns nil.
puts (a <=> "hello").inspect   # nil
puts (a <=> 100).inspect       # nil

# Type error on non-numeric `Time.at`.
begin
  Time.at("not-a-number")
rescue TypeError => e
  puts "TE: caught"
end

# UTC helpers — all no-ops in Tier 1 (we're always UTC).
puts t.utc?                    # true
puts t.zone                    # "UTC"
puts t.utc_offset              # 0
puts t.utc.equal?(t)           # true (returns self)

# eql? matches == for Time-vs-Time identity.
c = Time.at(100, 500_000)
d = Time.at(100, 500_000)
puts c.eql?(d)                 # true
puts c.eql?(Time.at(100))      # false (different usec)

# to_s memo isolation (preamble memoizes the rendered form): each
# call must return a FRESH string, and mutating a returned string
# must not pollute later calls. CRuby trivially satisfies this
# (no memo); the fixture pins the rubyrs memo's dup-out contract.
e = Time.at(86_400)
s1 = e.to_s
s2 = e.to_s
puts s1.equal?(s2)             # false (fresh object per call)
s1 << " MUTATED"
puts e.to_s                    # unpolluted
puts e.inspect                 # alias shares the memo, same form
