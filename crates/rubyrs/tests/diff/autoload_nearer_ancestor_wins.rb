# CRuby ancestry order: a constant AUTOLOADED on a NEARER ancestor wins over one
# already DEFINED on a FARTHER ancestor. `OrdC < OrdB < OrdA`, `OrdA::X` defined
# but `OrdB::X` autoloaded → `OrdC::X` fires OrdB's autoload and resolves to it,
# NOT OrdA::X. (zeitwerk test_ancestors "even if present above".)
require "fileutils"
dir = File.join(__dir__, "ord_tmp_xz")
FileUtils.mkdir_p(dir)
File.write(File.join(dir, "bx.rb"), "class OrdB; X = :B_loaded; end")

class OrdA; X = :A_defined; end
class OrdB < OrdA; end
class OrdC < OrdB; end
OrdB.autoload(:X, File.join(dir, "bx.rb"))

p OrdA::X    # :A_defined
p OrdC::X    # :B_loaded — OrdB (nearer) autoload beats OrdA (farther) defined
FileUtils.rm_rf(dir)
