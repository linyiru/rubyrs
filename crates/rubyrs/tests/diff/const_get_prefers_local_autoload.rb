# `Mod.const_get(:Hash, false)` must fire Mod's OWN pending autoload for
# `:Hash` rather than returning the toplevel `::Hash`. Surfaced by
# zeitwerk eager_load, which does `Namespace.const_get(:Hash, false)` on
# modules whose files (hash.rb/module.rb/string.rb) shadow core classes.
target = "/tmp/rubyrs_cg_local.rb"
File.write(target, "module Wrap\n  class Hash\n    def self.tag = :local_hash\n  end\nend\n")
module Wrap; end
Wrap.autoload(:Hash, target)
got = Wrap.const_get(:Hash, false)
puts got
puts got.tag
