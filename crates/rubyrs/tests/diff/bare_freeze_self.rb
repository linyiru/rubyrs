# Bare `freeze` (implicit self) inside a method body freezes self —
# `Object#freeze` is a universal dispatch arm, not a method-table entry,
# so the bare/no-receiver path must handle it too. Surfaced by erubi's
# `Engine#initialize` (`freeze` on the last line).
class Widget
  def initialize
    @x = 1
    freeze
  end
end
w = Widget.new
p w.frozen?
begin
  w.instance_variable_set(:@x, 2)
rescue => e
  p e.class
end
