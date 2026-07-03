# A class's OWN pending autoload for a constant must beat a same-named
# constant/autoload inherited from its SUPERCLASS. `Sub < Base`, `Base::X`
# already DEFINED, `Sub::X` AUTOLOADED → resolving `Sub::X` must fire Sub's
# own autoload (`:sub`), NOT return the inherited `Base::X` (`:base`).
#
# This is the START-scope twin of autoload_nearer_ancestor_wins.rb: there the
# winning autoload sits on a nearer ANCESTOR (handled by resolve_const_path's
# ancestor loop); here it sits on the class ITSELF, whose direct lookup used to
# probe only loaded consts and skip the pending autoload — so the ancestor walk
# wrongly fired Base::X. Reached via both a qualified read inside a class body
# (Op::LoadConstChain) and Module#const_get. (zeitwerk test_explicit_namespace
# "same cname in the superclass".)
require "fileutils"
dir = File.join(__dir__, "own_scope_tmp")
FileUtils.mkdir_p(dir)
File.write(File.join(dir, "sub_x.rb"), "Sub::X = :sub")

class Base; X = :base; end
class Sub < Base; end
Sub.autoload(:X, File.join(dir, "sub_x.rb"))

# const_get path
p Sub.const_get(:X)   # :sub

# qualified-read-inside-a-class-body path (compiles to LoadConstChain)
class Probe
  def self.read = Sub::X
end
p Probe.read          # :sub
p Base::X             # :base (unchanged)

FileUtils.rm_rf(dir)
