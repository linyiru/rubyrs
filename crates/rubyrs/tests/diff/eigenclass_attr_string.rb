## S3 item (d): `attr_*` with String (non-Symbol) args inside
## `class << self` — previously a hard parse error on the
## SELF-receiver desugar path ("attr_* with non-symbol args is not
## supported"). Such bodies now route to the real eigenclass-body
## path, where the runtime attr_* (self = the metaclass, shell
## redirect to real.singleton_*) coerces Strings like CRuby.

class Widget
  class << self
    attr_accessor "str"
    attr_reader "ro", :sym
    attr_writer "wo"
  end
end

Widget.str = 5
puts "str=#{Widget.str}"
Widget.instance_variable_set(:@ro, 1)
Widget.instance_variable_set(:@sym, 2)
puts "ro=#{Widget.ro}"
puts "sym=#{Widget.sym}"
Widget.wo = 9
puts "wo=#{Widget.instance_variable_get(:@wo)}"

## `class << Const` spelling (the ticket-2 arm — regression guard).
class Gadget; end
class << Gadget
  attr_accessor "gs"
end
Gadget.gs = "x"
puts "gs=#{Gadget.gs}"

## Non-eigenclass String attr control (regular class body).
class Plain
  attr_accessor "pa"
end
pl = Plain.new
pl.pa = 3
puts "pa=#{pl.pa}"

## Mixed with a following def in the same body — the whole body rides
## the real path; defs still land as class methods.
class Mixed
  class << self
    attr_accessor "level"
    def bump
      self.level = (level || 0) + 1
    end
  end
end
Mixed.bump
Mixed.bump
puts "mixed=#{Mixed.level}"
