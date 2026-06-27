# A bare constant assigned in a REGULAR class/module body is scoped to that
# module — CRuby never creates a TOP-LEVEL constant for `class Foo; X = 1; end`.
# (rubyrs's flat-const dual-write used to leak a top-level `X`, which then
# shadowed a later top-level `module X` in zeitwerk's reload tests.)
class Hotel; X = 1; end
p defined?(X)              # nil — no top-level X
p Hotel::X                # 1

module Lib
  Y = 100
  Z = Y + 1               # in-body bare read still resolves
  class Reader
    def get_y; Y; end     # method-body read resolves via the cref chain
  end
end
p defined?(Y)             # nil
p Lib::Y                  # 100
p Lib::Z                  # 101
p Lib::Reader.new.get_y   # 100

# A later top-level constant with a name used nested elsewhere is NOT shadowed:
class Outer; W = 7; end
W = 9
p W                       # 9 (top-level), not Outer's 7
