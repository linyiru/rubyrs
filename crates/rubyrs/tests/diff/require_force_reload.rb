# The dev-reload idiom `$LOADED_FEATURES.delete(path); require path`
# (sinatra/reloader, Rails dev-reload) must actually re-run the file.
# require dedups via $LOADED_FEATURES; deleting an entry forces a reload,
# while a normal repeat require still returns false. (Mid-load circular
# dedup is unaffected — a still-loading file is never treated as a forced
# reload.)
require "tmpdir"
require "fileutils"
dir = File.join(Dir.tmpdir, "rubyrs_force_reload_#{Process.pid}")
FileUtils.mkdir_p(dir)
path = File.join(dir, "feat.rb")
File.write(path, "$feat_loads = ($feat_loads || 0) + 1\n")

$feat_loads = 0
p require(path)        # true  (first load)
p $feat_loads          # 1
p require(path)        # false (dedup)
p $feat_loads          # 1

stored = $LOADED_FEATURES.grep(/feat\.rb\z/).last
$LOADED_FEATURES.delete(stored)
p require(path)        # true  (forced reload)
p $feat_loads          # 2
p require(path)        # false (dedup again)
p $feat_loads          # 2

FileUtils.rm_rf(dir)
