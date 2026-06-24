# require keys $LOADED_FEATURES (and Method#source_location) by CRuby's
# File.expand_path of the load path — absolute + `.`/`..` resolved but
# WITHOUT symlink resolution — not the realpath. So requiring through a
# symlink records the SYMLINK path, not its target. (rubyrs previously
# stored std::fs::canonicalize's realpath, e.g. /private/tmp on macOS,
# which broke `$LOADED_FEATURES.delete(path)` reload idioms.)
require "tmpdir"
require "fileutils"
dir = Dir.mktmpdir
begin
  real = File.join(dir, "real_feature.rb")
  File.write(real, "$loaded_via = __FILE__\n")
  link = File.join(dir, "link_feature.rb")
  File.symlink(real, link)

  p require(link)                          # true (first load)
  p $LOADED_FEATURES.include?(link)        # true  — symlink path (expand_path)
  p $LOADED_FEATURES.include?(real)        # false — NOT the realpath
  p $loaded_via == link                    # __FILE__ is the expand_path too
  p require(link)                          # false (dedup by the same key)
ensure
  FileUtils.rm_rf(dir)
end
