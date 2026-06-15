# A user Kernel#require override (zeitwerk decorates require to
# intercept loads) is invoked both when an autoload FIRES and on an
# explicit user-code `require`. An alias of the builtin require
# reaches the ORIGINAL builtin (CallBuiltinDirect) without re-entering
# the override — no infinite recursion.
$require_log = []
module Kernel
  alias_method :orig_require, :require
  def require(path)
    $require_log << File.basename(path)
    orig_require(path)
  end
end

# Autoload-fired require goes through the override.
target = "/tmp/rubyrs_diff_rovr_target.rb"
File.write(target, "RovrTarget = :loaded_via_override")
autoload :RovrTarget, target
p RovrTarget
p $require_log.include?("rubyrs_diff_rovr_target.rb")

# Explicit user-code require also goes through the override.
$require_log.clear
two = "/tmp/rubyrs_diff_rovr_two.rb"
File.write(two, "RovrTwo = 2")
require two
p RovrTwo
p $require_log.include?("rubyrs_diff_rovr_two.rb")

# Re-require of an already-loaded file returns false (still wrapped).
$require_log.clear
p require(two)
p $require_log.include?("rubyrs_diff_rovr_two.rb")

File.delete(target)
File.delete(two)
