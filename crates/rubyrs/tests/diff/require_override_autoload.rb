# A user Kernel#require override (zeitwerk decorates require to
# intercept autoloads) is invoked when an autoload FIRES, and an alias
# of the builtin require reaches the builtin without re-entering the
# override (no infinite recursion). (Explicit user-code `require`
# going through the override is a separate, not-yet-wired path.)
$require_log = []
module Kernel
  alias_method :orig_require, :require
  def require(path)
    $require_log << File.basename(path)
    orig_require(path)
  end
end

target = "/tmp/rubyrs_diff_rovr_target.rb"
File.write(target, "RovrTarget = :loaded_via_override")
autoload :RovrTarget, target
p RovrTarget
p $require_log.include?("rubyrs_diff_rovr_target.rb")
File.delete(target)
