# Integer#gcdlcm → [gcd, lcm] (both non-negative).
p 12.gcdlcm(8)
p 12.gcdlcm(0)
p 0.gcdlcm(5)
p 0.gcdlcm(0)
p 7.gcdlcm(13)
p (-12).gcdlcm(8)
p 12.gcdlcm(-8)
p 100.gcdlcm(80)
p 1.gcdlcm(1)
p 5.respond_to?(:gcdlcm)
begin; 12.gcdlcm(8.0); rescue => e; p e.class; end
begin; 12.gcdlcm; rescue => e; p e.class; end
