# CRuby's require accepts anything convertible via to_path/to_str (rb_get_path),
# not just String — a Pathname, or any object with #to_path. A missing target
# raises LoadError (the path WAS accepted), not ArgumentError/TypeError.
require "pathname"

begin
  require Pathname.new("/tmp/rubyrs_definitely_absent_xyz.rb")
rescue LoadError
  puts "pathname -> LoadError"
end

class HasToPath
  def to_path; "/tmp/rubyrs_also_absent_xyz.rb"; end
end
begin
  require HasToPath.new
rescue LoadError
  puts "to_path obj -> LoadError"
end
