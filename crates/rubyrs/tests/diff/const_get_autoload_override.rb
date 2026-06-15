# `const_get(name, false)` must fire a registered autoload through a user
# `Kernel#require` override — same as a bare constant reference. zeitwerk's
# eager_load descends implicit-namespace DIRECTORIES via
# `const_get(cname, false)`, relying on its decorated require to
# autovivify the module instead of file-loading the directory.
$cg_fired = []
$cg_dir = "/tmp/rubyrs_cg_autoload_ns"
system("rm", "-rf", $cg_dir)
Dir.mkdir($cg_dir)

module Kernel
  alias_method :__cg_orig_require, :require
  def require(path)
    if path == $cg_dir
      $cg_fired << path
      Object.const_set(:CgAutoNs, Module.new)
      return true
    end
    __cg_orig_require(path)
  end
end

Object.autoload(:CgAutoNs, $cg_dir)
got = Object.const_get(:CgAutoNs, false)
p got.is_a?(Module)
p $cg_fired == [$cg_dir]

system("rm", "-rf", $cg_dir)
