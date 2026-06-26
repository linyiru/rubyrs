# rubyrs's Random is MT19937, byte-compatible with CRuby: same seeding
# (init_genrand for 1-word seeds, init_by_array for wider), same integer
# rejection-bounding, same 53-bit float, same Fisher-Yates shuffle. These exact
# values are produced identically by MRI.
p Random.new(0).rand(1000)                      # 684
p (0...4).map { Random.new(0).rand(1000) }      # all 684 (fresh RNG each)
r = Random.new(0)
p (0...5).map { r.rand(1000) }                  # [684, 559, 629, 192, 835]
p Random.new(42).rand(100)                      # 51
p (Random.new(0).rand * 1e10).floor             # 5488135039 (0.5488135039…)
p [1,2,3,4,5,6,7,8,9,10].shuffle(random: Random.new(0))
p Random.new(0).rand(10..20)                    # within range, exact
p Random.new(2**40 + 7).rand(1000)              # init_by_array path
srand(0)
p [10,20,30,40,50].shuffle                      # default RNG seeded
