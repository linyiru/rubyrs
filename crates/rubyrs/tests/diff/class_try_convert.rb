# Hash.try_convert / Array.try_convert / String.try_convert — return the
# object when it's already the target type, its to_hash/to_ary/to_str
# coercion when it responds, else nil. (rake/task.rb:278
# `if opts = Hash.try_convert(args) and !opts.empty?`.)
p Hash.try_convert({"a" => 1})    # {"a"=>1}
p Hash.try_convert([])            # nil
p Hash.try_convert("x")           # nil
p Hash.try_convert(nil)           # nil
p Array.try_convert([1, 2])       # [1, 2]
p Array.try_convert("x")          # nil
p Array.try_convert(nil)          # nil
p String.try_convert("s")         # "s"
p String.try_convert(5)           # nil
# duck-typed to_hash / to_ary / to_str
h = Object.new
def h.to_hash; {"converted" => true}; end
p Hash.try_convert(h)             # {"converted"=>true}
a = Object.new
def a.to_ary; [9, 8]; end
p Array.try_convert(a)            # [9, 8]
s = Object.new
def s.to_str; "coerced"; end
p String.try_convert(s)           # "coerced"
