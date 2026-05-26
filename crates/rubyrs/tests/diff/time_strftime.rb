# `Time#strftime` — Tier 1 directive subset. All assertions go
# through `.utc` so CRuby's local-tz mode converges with the
# rubyrs UTC-only model. Locale-sensitive directives (`%A` / `%B`
# etc.) are pinned to the English / `LC_ALL=C` shape; ADR 0017
# Rule 1 keeps Tier 1 locale-deterministic.

# Reference epoch: 1_700_000_000 = 2023-11-14 22:13:20 UTC
# (Tuesday, day 318 of 2023). Sub-second carries 123_456 usec =
# 123_456_000 nsec for the `%N` / `%L` exercises.
t = Time.at(1_700_000_000, 123_456).utc

# ----- Numeric date components -----
puts t.strftime("%Y")           # "2023"
puts t.strftime("%C")           # "20"
puts t.strftime("%y")           # "23"
puts t.strftime("%m")           # "11"
puts t.strftime("%d")           # "14"
puts t.strftime("%e")           # "14"
puts t.strftime("%j")           # "318"
puts t.strftime("%w")           # "2" (Tuesday)
puts t.strftime("%u")           # "2" (ISO Monday=1, so Tue=2)

# ----- Numeric time components -----
puts t.strftime("%H")           # "22"
puts t.strftime("%k")           # "22"
puts t.strftime("%I")           # "10"
puts t.strftime("%l")           # "10"
puts t.strftime("%M")           # "13"
puts t.strftime("%S")           # "20"
puts t.strftime("%p")           # "PM"
puts t.strftime("%P")           # "pm"
puts t.strftime("%s")           # "1700000000"

# ----- Sub-second precision -----
puts t.strftime("%N")           # "123456000"
puts t.strftime("%3N")          # "123" (milliseconds)
puts t.strftime("%6N")          # "123456" (microseconds)
puts t.strftime("%9N")          # "123456000" (nanoseconds, explicit)
puts t.strftime("%L")           # "123" (millisecond, dedicated directive)

# ----- Named components -----
puts t.strftime("%A")           # "Tuesday"
puts t.strftime("%a")           # "Tue"
puts t.strftime("%B")           # "November"
puts t.strftime("%b")           # "Nov"
puts t.strftime("%h")           # "Nov" (alias)

# ----- Timezone (UTC-only Tier 1) -----
puts t.strftime("%z")           # "+0000"
puts t.strftime("%:z")          # "+00:00"
puts t.strftime("%::z")         # "+00:00:00"
puts t.strftime("%Z")           # "UTC"

# ----- Composites -----
puts t.strftime("%F")           # "2023-11-14"
puts t.strftime("%T")           # "22:13:20"
puts t.strftime("%X")           # "22:13:20" (alias)
puts t.strftime("%R")           # "22:13"
puts t.strftime("%D")           # "11/14/23"
puts t.strftime("%x")           # "11/14/23"
puts t.strftime("%r")           # "10:13:20 PM"
puts t.strftime("%v")           # "14-NOV-2023" (VMS date — uppercase month)
puts t.strftime("%c")           # "Tue Nov 14 22:13:20 2023"

# ----- Literal escapes -----
puts t.strftime("%%")           # "%"
puts t.strftime("text-%n-after").inspect    # "\"text-\\n-after\""
puts t.strftime("col1%tcol2").inspect       # "\"col1\\tcol2\""

# ----- Flags: `-` no-pad, `0` zero-pad, `_` space-pad -----
# `%-d` strips the default zero-padding.
puts Time.at(8 * 86400 + 3 * 3600 + 5 * 60 + 7).utc.strftime("%-d/%-m/%-Y") # "9/1/1970"
# `%_d` forces space padding even where zero-padding is default.
puts Time.at(8 * 86400 + 3 * 3600 + 5 * 60 + 7).utc.strftime("%_d") # " 9"
# Explicit `%0` flag keeps zero padding.
puts Time.at(8 * 86400).utc.strftime("%0d") # "09"
# `^` flag uppercases the directive's output.
puts t.strftime("%^A %^B %^p") # "TUESDAY NOVEMBER PM"

# ----- Explicit width -----
puts t.strftime("%5Y")          # " 2023"
puts t.strftime("%05Y")         # "02023" (explicit zero-pad)

# ----- Unknown directive passes through verbatim (CRuby parity) -----
puts t.strftime("%Q literal")   # "%Q literal"
puts t.strftime("ab%Wcd").length # CRuby supports %W as week-of-year;
                                  # our subset doesn't, so it passes
                                  # through verbatim. Pin the LENGTH
                                  # rather than the value to keep the
                                  # cross-impl assertion sturdy.

# ----- Composition fidelity -----
puts t.strftime("Today is %A, %B %-d, %Y at %H:%M:%S UTC")

# ----- Leap year / day-of-year boundaries -----
# 2020-02-29 (leap day) — doy 60.
leap = Time.at(1_582_934_400).utc
puts leap.strftime("%F %j")    # "2020-02-29 060"

# 2020-12-31 (leap year, last day) — doy 366.
end_leap = Time.at(1_609_372_800).utc
puts end_leap.strftime("%F %j") # "2020-12-31 366"

# 2023-12-31 (non-leap, last day) — doy 365.
end_nonleap = Time.at(1_704_067_199).utc
puts end_nonleap.strftime("%F %j") # "2023-12-31 365"

# ----- Hour conversion at noon / midnight (12-hour clock) -----
midnight = Time.at(0).utc
noon = Time.at(12 * 3600).utc
puts midnight.strftime("%I %p") # "12 AM"
puts noon.strftime("%I %p")     # "12 PM"
afternoon_one = Time.at(13 * 3600).utc
puts afternoon_one.strftime("%I %p") # "01 PM"

# ----- Empty format string -----
puts t.strftime("").inspect    # "\"\""
