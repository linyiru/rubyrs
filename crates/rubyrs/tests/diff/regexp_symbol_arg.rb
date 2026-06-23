# Regexp match family implicitly coerces a Symbol subject to its name
# String (CRuby): /re/.match?(sym) / .match(sym) / === sym. ActiveSupport's
# mattr_accessor validates attribute names with `/\A[_A-Za-z]\w*\z/
# .match?(sym)`.
p(/\A[_A-Za-z]\w*\z/.match?(:valid_name))   # true
p(/\A[_A-Za-z]\w*\z/.match?(:"1bad"))        # false
p(/\d/.match?(:abc))                          # false
p(/a/ === :cat)                               # true
p(/z/ === :cat)                               # false
m = /(\w)(\w)/.match(:hi)
p m[0]                                         # "hi"
p [m[1], m[2]]                                 # ["h", "i"]
p(/x/.match(:abc))                            # nil
