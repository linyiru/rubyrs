# The ASCII fast-path for String#[] (char index == byte index → direct
# byte slice) and the cached String#length must stay byte-identical to
# the generic char path. Long string so a perf regression that fell
# back to the O(n) lossy path would still be CORRECT here (this guards
# correctness; scaling is guarded separately).
s = "abcdefghij" * 5000   # 50_000 ASCII chars, UTF-8
p s.length                          # 50000
p s.size
p s.encoding.to_s                   # "UTF-8"
p s[0]                              # "a"
p s[49999]                          # "j"
p s[50000]                          # nil
p s[-1]                             # "j"
p s[-50000]                         # "a"
p s[10, 5]                          # "abcde"
p s[49998, 10]                      # "ij" (clamped)
p s[50000, 3]                       # "" (start==len)
p s[50001, 3]                       # nil
p s[5, -1]                          # nil
p s[10..14]                         # "abcde"
p s[-5..]                           # "fghij"
p s[..4]                            # "abcde"
p s[10...15]                        # "abcde"
p [s[3].encoding.to_s, s[3,2].encoding.to_s, s[3..5].encoding.to_s]  # all UTF-8
# US-ASCII tag preserved
u = "hello".encode("US-ASCII")
p [u[1,3], u[1,3].encoding.to_s]    # ["ell", "US-ASCII"]
# a multi-byte (non-ascii) string still works (generic path)
m = "héllo wörld" * 10
p m.length
p m[0, 5]
