# Integer.sqrt(n): exact integer square root (largest r with r*r <= n),
# exact even for Bignums; Float args are truncated to Integer first;
# negatives raise Math::DomainError.
p Integer.sqrt(0)
p Integer.sqrt(1)
p Integer.sqrt(15)
p Integer.sqrt(16)
p Integer.sqrt(17)
p Integer.sqrt(99)
p Integer.sqrt(625)
p Integer.sqrt(123456789012345)
p Integer.sqrt(10**20)
p Integer.sqrt(10**20) == 10**10
p Integer.sqrt(17.9)
p Integer.sqrt(16.0)

def t
  yield
rescue => e
  [e.class, e.message]
end
p t { Integer.sqrt(-1) }
