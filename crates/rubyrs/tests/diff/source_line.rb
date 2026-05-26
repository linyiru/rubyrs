# `__LINE__` — 1-based source line number of the literal.
# Pre-fix rubyrs returned `0` everywhere (a documented
# stub) because the AST translator didn't carry source
# bytes; Prism's `Location` exposes a raw start pointer
# but no pre-computed line. The fix threads the source
# slice through a thread-local + RAII guard
# (`ast::SourceGuard`) so the SourceLineNode arm can
# derive lines by counting `\n` in the prefix up to the
# location pointer.
#
# Lines below are pinned to specific numbers — touch them
# only when adjusting the fixture itself.

# Toplevel sequence.
puts __LINE__   # 15
puts __LINE__   # 16

# After a blank line + comment block — pin the offset.

puts __LINE__   # 21

# Inside a method body — `__LINE__` reports the line the
# literal appears in, regardless of where the method is
# called from.
def in_method
  __LINE__       # 28
end
puts in_method
puts __LINE__   # 31

# Inside a block — same per-literal rule.
[1].each do
  puts __LINE__ # 35
end

# Inside a class body.
class C
  puts __LINE__  # 40
  def self.where
    __LINE__     # 42
  end
end
puts C.where
