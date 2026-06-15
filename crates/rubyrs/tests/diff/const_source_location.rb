# Module#const_source_location(name): [file, line] for a Ruby-defined
# constant, [] for a C-defined (preamble/core) one, nil for an
# untriggered-autoload / undefined constant. Recorded at
# class/module/value-constant definition. zeitwerk reads it (and its
# suite asserts the location of an explicit-namespace class).

class TopClass; end
module Outer
  class Inner; end
  VAL = 99
end
TOP_VAL = [1, 2]

# Ruby-defined → [file, line] (basename + line, abspath is env-specific).
fc = Object.const_source_location(:TopClass)
p [File.basename(fc[0]), fc[1]]
p Object.const_source_location("Outer::Inner")[1]
p Outer.const_source_location(:Inner)[1]
p Outer.const_source_location(:VAL)[1]
p Object.const_source_location(:TOP_VAL)[1]

# Reopening does NOT move the location (first definition wins).
class TopClass; def x; end; end
p Object.const_source_location(:TopClass)[1]

# C-defined core → [] ; undefined → nil (no NameError).
p Object.const_source_location(:String)
p Object.const_source_location(:Comparable)
p Object.const_source_location(:DefinitelyNotAConst)
p Outer.const_source_location(:NopeNotHere)

# String / Symbol name forms agree.
p Object.const_source_location("TopClass")[1]
