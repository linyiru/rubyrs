# String#[]= with a substring key replaces the first occurrence
# (IndexError if absent), alongside the existing index/range forms.
s = "/src/foo/bar"; s["/src/"] = ""; p s
t = "hello world"; t["world"] = "there"; p t
u = "abcabc"; u["bc"] = "X"; p u
v = "abc"; begin; v["zzz"] = "Q"; rescue => e; p [e.class, e.message]; end
w = "0123"; w[1] = "X"; p w                     # int index still works
x = "0123"; x[1, 2] = "YY"; p x                 # start,len still works
