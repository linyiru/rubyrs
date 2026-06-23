# Integer#[](range) — the Range form of bit access (Ruby 2.7+).
# `n[i..j]` / `n[i...j]` extract the bitfield from bit i to j; an
# endless range is the full arithmetic right shift. A beginless range
# raises ArgumentError. Byte-stable against CRuby.

n = 0b101101  # 45
p n[1..3]      # 6
p n[1...4]     # 6
p n[2..]       # 11  (45 >> 2)
p n[0..]       # 45
p n[2...5]     # 3
p n[10..20]    # 0
p n[64..70]    # 0
p n[3..1]      # 5   (begin > end → no mask, just 45 >> 3)
p n[1..-1]     # 22  (negative computed length → no mask)

# Negative receivers sign-extend (arithmetic shift).
p (-45)[1..3]  # 1
p (-45)[2..]   # -12

begin
  n[..3]
rescue => e
  puts "#{e.class}: #{e.message}"  # ArgumentError beginless
end
