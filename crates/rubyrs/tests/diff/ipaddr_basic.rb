# Pure-Ruby IPAddr (Tier 3): IPv4/IPv6 + CIDR + include?/===.
# rack-protection's HostAuthorization: IPAddr.new(cidr).include?(host).
require "ipaddr"
p IPAddr::Error.ancestors.include?(ArgumentError)
p IPAddr::InvalidAddressError.ancestors.include?(IPAddr::Error)
p IPAddr.new("0.0.0.0/0").include?("127.0.0.1")
p IPAddr.new("::/0").include?("127.0.0.1")
p IPAddr.new("::/0").include?("::1")
p IPAddr.new("127.0.0.0/8").include?("127.0.0.1")
p IPAddr.new("127.0.0.0/8").include?("128.0.0.1")
p IPAddr.new("10.0.0.0/255.255.255.0").include?("10.0.0.5")
p IPAddr.new("10.0.0.0/255.255.255.0").include?("10.0.1.5")
p IPAddr.new("192.168.1.0/24").include?(IPAddr.new("192.168.1.99"))
p IPAddr.new("1.2.3.4").to_s
p IPAddr.new("2001:db8::1").to_s
p IPAddr.new("[::1]").to_s
p IPAddr.new("1.2.3.0/24").prefix
p IPAddr.new("2001:db8::/32").prefix
p IPAddr.new("1.2.3.4").ipv4?
p IPAddr.new("::1").ipv6?
p(IPAddr.new("1.2.3.4") == IPAddr.new("1.2.3.4"))
p(IPAddr === IPAddr.new("1.2.3.4"))
begin; IPAddr.new("example.com"); rescue => e; p e.class; end
begin; IPAddr.new("0.0.0.0/0").include?("nope"); rescue => e; p e.class; end
begin; IPAddr.new("1.2.3.4/99"); rescue => e; p e.class; end
