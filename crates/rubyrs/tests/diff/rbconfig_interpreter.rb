# RbConfig interpreter-path keys are present + sanely shaped (the exact
# paths differ per interpreter, so assert invariants that hold for both:
# EXEEXT is "" on POSIX, the dir/name keys are non-empty Strings, and
# RbConfig.ruby is a non-empty String path to the running interpreter).
require "rbconfig"
p RbConfig::CONFIG["EXEEXT"]
p RbConfig::CONFIG["bindir"].is_a?(String)
p !RbConfig::CONFIG["bindir"].empty?
p RbConfig::CONFIG["ruby_install_name"].is_a?(String)
p !RbConfig::CONFIG["ruby_install_name"].empty?
p RbConfig.ruby.is_a?(String)
p !RbConfig.ruby.empty?
