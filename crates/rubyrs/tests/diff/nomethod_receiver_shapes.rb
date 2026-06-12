# NoMethodError receiver descriptions — CRuby 3.4 shapes:
# literal singletons render as themselves ("for nil", "for true"),
# instances as "an instance of X", Class/Module receivers as
# "for class X" / "for module X" (unquoted).
#
# The method-name QUOTE style still differs (we keep the legacy
# `name' backticks; CRuby 3.4 uses 'name'), so messages are
# normalized via tr before printing — receiver shape is what this
# fixture pins. (Quote-style alignment is a separate sweep.)

def msg
  yield
  "NO-RAISE"
rescue NoMethodError => e
  e.message.tr("`", "'")
end

puts msg { nil.zork }
puts msg { true.zork }
puts msg { false.zork }
puts msg { 42.zork }
puts msg { 4.2.zork }
puts msg { :sym.zork }
puts msg { "s".zork }
puts msg { [].zork }
puts msg {({}.zork) }
puts msg { String.zork }

module ShapeProbeM; end
puts msg { ShapeProbeM.zork }

class ShapeProbeC; end
puts msg { ShapeProbeC.new.zork }

# undef_method / alias_method NameError naming (straight quotes on
# both sides — byte-comparable): a singleton-of-class shell names
# the ATTACHED constant for undef_method, but keeps the eigenclass
# display name for alias_method. minitest's Object#stub surfaces
# the undef form (its ensure-block undef_method replaces the
# in-flight alias NameError).
def nmsg
  yield
  "NO-RAISE"
rescue NameError => e
  e.message
end

class ShapeProbeT; end
puts(nmsg { ShapeProbeT.undef_method(:nope) })
puts(nmsg { class << ShapeProbeT; undef_method(:nope); end })
puts(nmsg { class << ShapeProbeT; alias_method(:x, :nope); end })

module ShapeProbeMM; end
puts(nmsg { ShapeProbeMM.undef_method(:nope) })
puts(nmsg { class << ShapeProbeMM; undef_method(:nope); end })
