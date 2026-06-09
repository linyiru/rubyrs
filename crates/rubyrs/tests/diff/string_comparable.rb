# String includes Comparable → #between? / #clamp (built on the native
# <=>); native ==/<=>/sort fast paths are unaffected.
p "b".between?("a", "c")
p "a".between?("b", "z")
p "m".clamp("a", "z")
p "z".clamp("a", "m")        # above hi → hi
p "0".clamp("a", "z")        # below lo → lo
p "m".clamp("n".."z")        # Range form
p "m".clamp("a".."k")
p "x".respond_to?(:clamp)
p "x".respond_to?(:between?)
# native comparison still correct
p "abc" == "abc"
p "abc" == 5
p("a" < "b")
p ["banana", "apple", "cherry"].sort
p ["b", "a", "c"].min
p ["b", "a", "c"].max
