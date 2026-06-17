# Float#to_int — CRuby's implicit-conversion alias of to_i (truncates
# toward zero; FloatDomainError on NaN/Infinity). respond_to? agrees.
p 17.9.to_int
p (-3.7).to_int
p 5.0.to_int
p 0.0.to_int
p 17.9.respond_to?(:to_int)
p [1.5, 2.9].map(&:to_int)
def t
  yield
rescue => e
  e.class
end
p t { (1.0 / 0).to_int }
p t { (0.0 / 0).to_int }
