# File.read with the "bom|utf-8" external-encoding option strips a
# leading UTF-8 BOM (NOTE: that option string must NOT appear on this
# file's first line — CRuby would parse it as an encoding magic
# comment); without the bom| prefix the BOM is content. Discovered by
# the front-matter differential: a BOM-prefixed post matched Jekyll's
# YAML_FRONT_MATTER_REGEXP on CRuby (whose Document#read_content reads
# with merged_file_read_opts → "bom|utf-8") but not on rubyrs, which
# kept the BOM bytes.
#
# Stages fixed paths under /tmp (the diff harness runs both engines
# on the same machine; same idiom as kernel_load.rb).
BOM_PATH   = "/tmp/rubyrs_bom_read_test.md"
PLAIN_PATH = "/tmp/rubyrs_plain_read_test.md"
File.open(BOM_PATH, "wb")   { |f| f.write("\xEF\xBB\xBF---\na: 1\n---\nbody\n") }
File.open(PLAIN_PATH, "wb") { |f| f.write("---\na: 1\n---\nbody\n") }

[["bom", BOM_PATH], ["plain", PLAIN_PATH]].each do |kind, path|
  raw      = File.read(path)
  stripped = File.read(path, :encoding => "bom|utf-8")
  str_key  = File.read(path, "encoding" => "bom|utf-8")
  caps     = File.read(path, :encoding => "BOM|UTF-8")
  plainenc = File.read(path, :encoding => "utf-8")
  puts "#{kind}: raw         starts_dash=#{raw.start_with?("---")} bytes=#{raw.bytesize}"
  puts "#{kind}: bom|utf-8   starts_dash=#{stripped.start_with?("---")} bytes=#{stripped.bytesize}"
  puts "#{kind}: string-key  starts_dash=#{str_key.start_with?("---")} bytes=#{str_key.bytesize}"
  puts "#{kind}: case-insens starts_dash=#{caps.start_with?("---")} bytes=#{caps.bytesize}"
  puts "#{kind}: plain utf-8 starts_dash=#{plainenc.start_with?("---")} bytes=#{plainenc.bytesize}"
  fm = File.read(path, :encoding => "bom|utf-8") =~ /\A(---\s*\n.*?\n?)^((---|\.\.\.)\s*$\n?)/m
  puts "#{kind}: front-matter match=#{!!fm}"
end

File.delete(BOM_PATH, PLAIN_PATH)
