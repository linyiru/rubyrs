# Regenerate the pre-extracted carmine lexer tables embedded by the
# `_rouge_native` static fast path (src/rouge_tables/*.json).
#
# The tables are extracted from a SPECIFIC rouge version; the kramdown
# shim's version gate (STATIC_HL_ROUGE_VERSION in
# kramdown_native_shim.rb) must match, so bump both together when
# upgrading rouge.
#
# Usage (needs a rubyrs binary built with _rouge_native and a gem dir
# containing the target rouge):
#   target/release/rubyrs crates/rubyrs/tools/dump_rouge_static_tables.rb \
#     /path/to/gems crates/rubyrs/src/rouge_tables
gems_dir = ARGV[0] or abort "usage: dump_rouge_static_tables.rb GEMS_DIR OUT_DIR"
out_dir = ARGV[1] or abort "usage: dump_rouge_static_tables.rb GEMS_DIR OUT_DIR"
$LOAD_PATH.unshift(*Dir["#{gems_dir}/gems/*/lib"])
require "rouge"
require "json"
cn = Rouge::CarmineNative
abort "rouge_native shim not active (need a _rouge_native binary)" unless defined?(cn::Recorder)
puts "rouge version: #{Rouge.version}"
{ "python" => Rouge::Lexers::Python,
  "ruby" => Rouge::Lexers::Ruby,
  "bash" => Rouge::Lexers::Shell }.each do |lang, lc|
  states = {}
  upgrade = cn::WORDLIST_BUILDERS[lc.name]
  trust = cn::TRACE_ALLOWLIST[lc.name] ? true : false
  lc.state_definitions.each do |name, dsl|
    defn = dsl.instance_variable_get(:@defn)
    abort "#{lang}: missing state defn #{name}" unless defn
    rec = cn::Recorder.new(upgrade, trust)
    rec.instance_eval(&defn)
    states[name.to_s] = rec.rules
  end
  json = JSON.generate({ "states" => states, "shortnames" => cn.send(:shortnames) })
  File.write("#{out_dir}/#{lang}.json", json)
  puts "#{lang}: #{json.bytesize} bytes"
end
