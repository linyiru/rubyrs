# `require 'ipaddr'` — Tier 1 lenient stub. Sinatra 4 +
# rack-protection 4 require it at module-load time. Class-body
# usage in those gems is constant-shape only (`when IPAddr`,
# `rescue IPAddr::InvalidAddressError`); `IPAddr.new(...)` calls
# are inside lambdas that run later. The bare constant shell
# is enough to clear the require.
#
# Discovery: P3 Sinatra spike — sinatra/base.rb:17 raised
# `LoadError: cannot load such file -- ipaddr`.

# Shape 1: require returns true on first load, false on
# subsequent (CRuby loaded-features dedup semantics).
puts "first=#{require 'ipaddr'}"
puts "second=#{require 'ipaddr'}"

# Shape 2: IPAddr constant is materialised as a Class.
puts "is_class=#{IPAddr.is_a?(Class)}"
puts "defined=#{defined?(IPAddr) ? 'constant' : 'nil'}"

# Shape 3: feature-absent surface — calling .new still raises
# (Tier 1 keeps the "no methods" behaviour; full IPAddr stays
# behind --features stdlib if/when added).
begin
  IPAddr.new("0.0.0.0/0")
rescue NoMethodError => e
  puts "absent=NoMethodError"
end

# Shape 4: `when IPAddr` in a case shape — IPAddr.=== falls
# through to Class#=== (is_a?), so non-IPAddr scrutinees skip
# this branch cleanly. Matches what rack-protection's host
# authorization does at class-body time.
result = case "not-an-ipaddr"
         when IPAddr then "matched"
         else "default"
         end
puts "case=#{result}"
