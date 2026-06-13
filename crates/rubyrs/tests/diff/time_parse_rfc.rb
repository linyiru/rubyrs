# `Time.parse` accepts the RFC 2822 / RFC 7231 httpdate shapes — the
# month-NAME layout ("Sat, 13 Jun 2026 22:00:05 GMT") — not just the
# ISO numeric `-` form. rack's Response cache helpers write
# `Time.now.httpdate` into a header then re-`Time.parse` it; before
# this `Time.parse` raised "no time information" on its own output.
# All cases here carry an explicit zone (GMT / +0000 / Z) so the
# result is absolute and TZ-independent.
require "time"

[
  "Sat, 13 Jun 2026 22:00:05 GMT",
  "Sun, 06 Nov 1994 08:49:37 GMT",
  "Mon, 01 Jan 2001 00:00:00 GMT",
  "13 Jun 2026 22:00:05 +0000",          # rfc2822 without the Dow prefix
  "Tue, 15 Nov 1994 12:45:26 +0000",
  "Wed, 31 Dec 1969 23:59:59 GMT",
  "2026-06-13T22:00:05Z",                # ISO with Z — unchanged path
  "2026-06-13T22:00:05+00:00",
].each do |s|
  # Compare the absolute instant only. The `utc?` flavour bit is a
  # documented Tier-1 UTC-only modelling nuance (rubyrs's rfc2822
  # path returns a utc-flavoured Time; CRuby's Time.parse returns a
  # local-flavoured one at the same offset) — the instant is what
  # callers compare.
  t = Time.parse(s)
  puts "#{s} => #{t.to_i}"
end

# Round-trip: httpdate output re-parses to the same instant.
base = Time.utc(2026, 6, 13, 22, 0, 5)
puts base.httpdate
puts(Time.parse(base.httpdate).to_i == base.to_i)

# A non-date string still raises (month name absent → ISO path).
begin
  Time.parse("not a date")
  puts "no raise"
rescue ArgumentError
  puts "ArgumentError"
end
