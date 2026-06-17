# `require "foo/1.0"` must look for `foo/1.0.rb` — CRuby appends `.rb`
# to the feature name unless it already ends in `.rb`; a trailing dotted
# segment like `.0` is NOT a pre-existing extension. Surfaced by the rss
# gem (`require "rss/1.0"`, "rss/2.0", …).
base = "/tmp/rubyrs_reqdot_#{Process.pid}"
sub = "#{base}/pkg"
Dir.mkdir(base) unless Dir.exist?(base)
Dir.mkdir(sub) unless Dir.exist?(sub)
begin
  File.write("#{sub}/1.0.rb", "PKG_ONE_OH = :v10\n")
  File.write("#{sub}/2.0.rb", "PKG_TWO_OH = :v20\n")
  File.write("#{sub}/data.json.rb", "JSON_SHIM = :js\n")
  $LOAD_PATH.unshift(base)

  p require("pkg/1.0")          # true
  p PKG_ONE_OH                  # :v10
  p require("pkg/2.0")          # true
  p PKG_TWO_OH                  # :v20
  p require("pkg/1.0")          # false (already loaded)

  # a name whose trailing segment isn't .rb still gets .rb appended
  p require("pkg/data.json")    # true
  p JSON_SHIM                   # :js
ensure
  [ "#{sub}/1.0.rb", "#{sub}/2.0.rb", "#{sub}/data.json.rb" ].each { |f| File.delete(f) if File.exist?(f) }
  Dir.rmdir(sub) if Dir.exist?(sub)
  Dir.rmdir(base) if Dir.exist?(base)
end
