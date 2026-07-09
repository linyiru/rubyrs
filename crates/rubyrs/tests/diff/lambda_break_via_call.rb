# `break` from a LAMBDA invoked via `.call` / `.()` / `[]` is a LOCAL
# return of the block value — `-> { break 5 }.call` yields 5. A proc's
# `break` in the same position is still a LocalJumpError
# ("break from proc-closure"). Previously rubyrs raised LocalJumpError
# for the lambda case too (the invoke path didn't distinguish lambda
# from proc for `break`). `next` / `return` are unchanged for both.

def show
  yield
rescue LocalJumpError => e
  puts "LocalJumpError: #{e.message}"
end

# LAMBDA break → local return of the value
p(-> { break 5 }.call)          # 5
p(-> { break }.call)            # nil  (no value → nil)
p(-> { break 5 }.())            # 5  (.() form)
p(-> { break 5 }[])             # 5  ([] form)
p(-> { break 3; 99 }.call)      # 3  (code after break is dead)
p(->(a) { break a * 2 }.call(6))# 12 (with an arg)

# proc / Proc.new break → LocalJumpError (UNCHANGED)
show { p(proc { break 5 }.call) }       # break from proc-closure
show { p(Proc.new { break 7 }.call) }   # break from proc-closure

# next / return unchanged in BOTH
p(-> { next 7 }.call)           # 7
p(proc { next 8 }.call)         # 8
p(-> { return 9 }.call)         # 9  (lambda return is local)

# break targeting an INNER iterator inside a lambda still breaks the
# iterator, and the lambda returns normally (not a local-return of the
# inner break value unless it falls through).
p(-> { [1, 2, 3].each { |x| break x if x == 2 } }.call)  # 2
