# Runtime-aware loader for JSON::Ext::Parser.
#
# On rubyrs the C parser is the flori-json-cext example bundle
# (built via `bash crates/rubyrs/examples/flori-json-cext/build.sh`);
# the harness sets `RUBYRS_FLORI_JSON_CEXT_DIR` to its absolute path
# so this shim can `require` it without depending on $LOAD_PATH
# config.
#
# On CRuby the same `JSON::Ext::Parser` class ships in stdlib's
# `json/ext` (loaded transitively by `require "json"`, but we ask
# for the cext face directly so the shape matches the rubyrs path).
if defined?(RUBYRS)
  bundle_dir = ENV["RUBYRS_FLORI_JSON_CEXT_DIR"] or
    raise "RUBYRS_FLORI_JSON_CEXT_DIR not set — harness should export this for fixtures with required_cext_examples"
  require "#{bundle_dir}/parser"
else
  require "json/ext"
end
