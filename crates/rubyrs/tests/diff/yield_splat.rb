# `yield(*x)` / `yield(a, *b, c)` — splat in a yield. Compiles to
# Op::ApplyYield (the yield analogue of Op::ApplyCall): the combined args
# Array is expanded onto the stack and the block runs with the dynamic
# argc. (Previously rejected at AST translation: "unsupported node:
# SplatNode".)
def f1; yield(*[1, 2]); end
f1 { |a, b| p [a, b] }                 # [1, 2]
def f3(arr); yield(*arr); end
f3([10, 20]) { |a, b| p a + b }        # 30
def f4; yield(0, *[1, 2], 9); end      # mixed
f4 { |*xs| p xs }                      # [0, 1, 2, 9]
def f5; [[1, 2], [3, 4]].each { |*x| yield(*x) }; end  # splat-yield in a block
f5 { |a, b| p [a, b] }                 # [1,2] then [3,4]
def f6; yield(*[]); end                # empty splat
f6 { p :called }                       # :called
def f7(a, rest); yield(a, *rest); end  # leading positional + splat local
f7(:head, [:b, :c]) { |*xs| p xs }     # [:head, :b, :c]
# return value of yield flows back
def f8; yield(*[3, 4]) * 2; end
p f8 { |a, b| a + b }                  # 14
# block_given? false path unaffected
def f9; block_given? ? yield(*[1]) : :no; end
p f9                                   # :no
