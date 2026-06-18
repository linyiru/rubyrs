# `}` (Hash-with-default) and `C` (Array/Hash subclass) marshal tags:
# byte-compatible dump + deep-copy round-trip.
hd = Hash.new(0); hd[:x] = 1
p Marshal.dump(hd).bytes
r = Marshal.load(Marshal.dump(hd))
p [r[:x], r[:missing], r.default]
# complex default round-trips
hd2 = Hash.new([1, 2]); hd2[:a] = 9
p Marshal.load(Marshal.dump(hd2)).default
# Array subclass → C wrapper
class MyArr < Array; end
a = MyArr.new; a << 1 << 2
p Marshal.dump(a).bytes
ra = Marshal.load(Marshal.dump(a))
p [ra.class.name, ra.to_a, ra.is_a?(Array)]
# Hash subclass → C wrapper
class MyHash < Hash; end
mh = MyHash.new; mh[:k] = 9
p Marshal.dump(mh).bytes
rh = Marshal.load(Marshal.dump(mh))
p [rh.class.name, rh[:k]]
# deep copy independence through a hash default's value graph
orig = Hash.new(0); orig[:list] = [1, 2]
copy = Marshal.load(Marshal.dump(orig))
copy[:list] << 9
p orig[:list]
