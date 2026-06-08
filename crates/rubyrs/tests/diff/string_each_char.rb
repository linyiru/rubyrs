# String#each_char — yield each character (as a 1-char String), return
# self; no-block → Enumerator (`.to_a` == `chars`). rouge's j lexer uses
# `k.each_char { |c| ... }`.
out = []
"héllo".each_char { |c| out << c }
p out                                  # ["h","é","l","l","o"]
p "abc".each_char { |c| }              # "abc" (returns self)
p "abc".each_char.to_a                 # ["a","b","c"]
p "abc".each_char.class.to_s           # "Enumerator"
p "abcd".each_char.map { |c| c.upcase } # ["A","B","C","D"]
p "x".each_char.with_index.to_a        # [["x",0]]
r = "abcd".each_char { |c| break c.upcase if c == "c" }
p r                                    # "C"
p "".each_char.to_a                    # []
# non-local return
def via_each_char(s)
  s.each_char { |c| return c if c == "b" }
  :none
end
p via_each_char("abc")                 # "b"
