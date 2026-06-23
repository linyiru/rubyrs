# IPAddr method-surface parity (vendored stdlib_vendor/ipaddr.rb).
# Covers the gaps filled alongside Date/DateTime: #succ, #private?,
# #loopback?, and the to_range endpoints. (Iterating to_range with
# #to_a needs generic succ-based Range#each, a separate VM feature
# not yet wired — so this fixture probes the endpoints, not .to_a.)
# Runs under --features stdlib with CRuby's core `ipaddr` as oracle.
require "ipaddr"

# Construction + the existing core surface (regression guard).
p IPAddr.new("192.168.1.1").to_s
p IPAddr.new("192.168.0.0/24").include?("192.168.0.5")
p IPAddr.new("2001:db8::1").to_s
p IPAddr.new("192.168.1.130").mask(24).to_s

# succ.
p IPAddr.new("10.0.0.5").succ.to_s
p IPAddr.new("192.168.0.255").succ.to_s
p IPAddr.new("2001:db8::ff").succ.to_s

# to_range endpoints (begin / end / first / last).
r = IPAddr.new("192.168.0.0/30").to_range
p [r.first.to_s, r.last.to_s, r.begin.to_s, r.end.to_s]

# private? across the RFC 1918 / fc00::/7 boundaries.
%w[10.0.0.1 172.16.0.1 172.32.0.1 192.168.1.1 169.254.0.1 8.8.8.8 fc00::1 fe80::1].each do |a|
  print IPAddr.new(a).private?, " "
end
puts

# loopback? across 127.0.0.0/8 and ::1.
%w[127.0.0.1 127.5.5.5 ::1 192.168.1.1 2001:db8::1].each do |a|
  print IPAddr.new(a).loopback?, " "
end
puts
