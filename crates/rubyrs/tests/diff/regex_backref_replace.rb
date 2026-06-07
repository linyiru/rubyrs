# gsub/sub replacement backreferences: numbered refs abutting an
# alphanumeric must NOT be swallowed into the group name, and the
# `\k<name>` named form must resolve. Regression: the bare `$1`
# translation parsed `\1X` as a group named "1X" (empty).
puts "abc".gsub(/(b)/, '\1X')                                   # abXc
puts "abc".sub(/(b)/, '\1X')                                    # abXc
puts "a1b2".gsub(/([a-z])(\d)/, '\2\1s')                        # 1as2bs
puts "hello world".gsub(/(\w)(\w+)/, '\1.\2')                   # h.ello w.orld
puts "2024-06".gsub(/(?<y>\d+)-(?<m>\d+)/, '\k<m>/\k<y>')       # 06/2024
puts "2024".gsub(/(?<y>\d+)/, '\k<y>!')                         # 2024!
puts "x".gsub(/x/, '\&\&')                                      # xx
puts "price $5".gsub(/\$(\d)/, '[\1]')                          # price [5]
puts "ab".gsub(/(a)(b)/, '\0=\1\2')                             # ab=ab
