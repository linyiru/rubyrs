# Array#join works at the byte level with encoding negotiation: a
# binary (ASCII-8BIT) element with bytes >127 must concatenate verbatim,
# NOT be re-encoded to UTF-8 (which doubled each high byte). The plain
# text-join path and the result encoding are unchanged.

# Binary elements — bytes preserved, result is ASCII-8BIT.
p ["\xc8".b, "\xff".b].join.bytesize
p ["\xc8".b, "\xff".b].join.encoding.to_s
p ["\xc8".b, "\xff".b].join.bytes
p [200, 10, 255].map { |b| b.chr }.join.bytesize
p (200.chr + 10.chr).bytesize

# Plain ASCII text join — unchanged (UTF-8 result).
p ["a", "b", "c"].join
p ["a", "b", "c"].join("-")
p ["a", "b"].join.encoding.to_s

# Numbers, symbols, nested arrays still stringify / recurse.
p [1, 2, [3, 4]].join("-")
p [1, "x", :y].join
p [[1, 2], [3, [4, 5]]].join(",")

# UTF-8 multibyte elements concatenate by bytes (no corruption).
p ["é", "ü"].join.bytesize
p ["é", "ü"].join.encoding.to_s

# ASCII-only binary element stays compatible with text.
p ["a", "b".b].join.bytesize
p ["a", "b".b].join.encoding.to_s
