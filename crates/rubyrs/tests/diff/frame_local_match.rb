# Frame-local `$~`: a method gets its own match data. A regex match a
# callee runs internally must NOT leak back into the caller's $~ / $1..
# (CRuby makes $~ method-local; blocks transparently share the
# enclosing method's $~).

# 1. nested normal-return call keeps the caller's match intact
def inner; "zzz" =~ /(z+)/; end
def outer
  "foo == bar" =~ /(\w+) (==) (\w+)/
  inner
  [$1, $2, $3]
end
p outer

# 2. block transparently shares + mutates the enclosing method's $~
def block_share
  "abc" =~ /(a)/
  [1].each { "xyz" =~ /(x)/ }
  $1
end
p block_share

# 3. a fresh method sees nil $~ (method-local, no match yet)
"top" =~ /(t)/
def fresh; $1; end
p fresh

# 4. callee's match restored after return; caller's groups survive
def helper; "999" =~ /(\d)/; $1; end
def caller_m
  "ab" =~ /(a)(b)/
  h = helper
  [$1, $2, h]
end
p caller_m

# 5. non-local `return` out of a block still keeps the match method-local
def inner_ret; "zzz" =~ /(z)/; [1].each { return $1 }; 99; end
def outer_ret
  "ab" =~ /(a)(b)/
  inner_ret
  [$1, $2]
end
p outer_ret

# 6. exception unwind: rescue body sees the handler method's $~
def raiser; "zzz" =~ /(z)/; raise "x"; end
def catcher
  "ab" =~ /(a)(b)/
  begin; raiser; rescue; end
  [$1, $2]
end
p catcher
