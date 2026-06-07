# MatchData#[] indexes the conceptual array [whole, *caps]:
# integer (incl. negative), Range, and named lookups. Regression:
# negative indices were off by one (`@caps[i - 1]` → m[-1] gave the
# second-to-last capture instead of the last).
m = "foobar".match(/(o+)(b)(a)/)   # whole "ooba"; caps oo, b, a
p m[0]
p m[1]
p m[-1]
p m[-2]
p m[-3]
p m[-4]
p m[-5]
p m[3]
p m[4]
p m[1..2]
p m[-2..]

m2 = "2024-06".match(/(?<y>\d+)-(?<mo>\d+)/)
p m2[:y]
p m2["mo"]
p m2[-1]
begin
  m2[:nope]
rescue IndexError => e
  puts "IndexError: #{e.message}"
end
