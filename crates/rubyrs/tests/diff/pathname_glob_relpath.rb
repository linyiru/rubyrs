# Pathname#/, Pathname#relative_path_from, Pathname.glob — the trio
# rouge's `load_lexers` uses (rouge.rb:49-54):
#   lexer_dir = Pathname.new(LIB_DIR) / "rouge/lexers"
#   Pathname.glob(lexer_dir / '*.rb').each { |f|
#     Lexers.load_lexer(f.relative_path_from(lexer_dir)) }
require "pathname"

# Pathname#/ (deterministic path-string join)
p (Pathname.new("/a/b") / "c/d").to_s            # "/a/b/c/d"
p (Pathname.new("rouge") / "lexers").to_s        # "rouge/lexers"

# relative_path_from — pure string, no filesystem
p Pathname.new("a/b/c").relative_path_from(Pathname.new("a/b")).to_s     # "c"
p Pathname.new("a/b/c").relative_path_from(Pathname.new("a/x")).to_s     # "../b/c"
p Pathname.new("/u/l/rouge/lexers/ruby.rb").relative_path_from(
    Pathname.new("/u/l/rouge/lexers")).to_s                              # "ruby.rb"
p Pathname.new("/a/b").relative_path_from(Pathname.new("/a/b")).to_s     # "."
p Pathname.new("a").relative_path_from("a/b/c").to_s                     # "../.."
# String base coerces to Pathname.
p Pathname.new("x/y/z.rb").relative_path_from("x/y").to_s                # "z.rb"
# Mismatched absolute/relative prefix raises ArgumentError.
begin
  Pathname.new("/abs").relative_path_from(Pathname.new("rel"))
rescue ArgumentError
  p :diff_prefix
end

# Pathname.glob: returns Pathname objects; empty match is []. (A real
# match is filesystem-dependent and verified separately.)
g = Pathname.glob("/nonexistent_rubyrs_xyz_dir/*.rb")
p g                                              # []
p g.class                                        # Array
# block form returns nil, yields nothing for an empty match.
p(Pathname.glob("/nonexistent_rubyrs_xyz_dir/*.rb") { |f| f })  # nil
