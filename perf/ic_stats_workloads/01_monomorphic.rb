# Monomorphic receiver — single class shape on the hot dispatch
# site. Expected: hit rate ~ 0.999 (every call hits the IC after
# the first miss).
N = 10_000
class Mono
  def ping
    42
  end
end
m = Mono.new
total = 0
i = 0
while i < N
  total += m.ping
  i += 1
end
puts total
