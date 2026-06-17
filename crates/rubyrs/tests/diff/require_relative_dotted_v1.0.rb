# Helper for require_relative_dotted.rb. Its basename ends in
# ".0" (a non-loadable extension), so require_relative must still
# append ".rb" to find it — mirroring rss's `require_relative
# "maker/1.0"`.
DOTTED_LOADED = "v1.0 loaded"
def dotted_greet; "from dotted helper"; end
