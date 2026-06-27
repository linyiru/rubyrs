# File.realpath resolves symlinks to a canonical absolute path (the filesystem-
# touching counterpart of the purely-lexical expand_path). Compared live against
# CRuby on the same machine, so exact resolved paths match.
p File.realpath("/")                       # "/"
p File.realpath("/tmp").start_with?("/")   # true (absolute)
p File.realpath("/tmp") == File.realpath("/tmp")  # true (stable)
p File.realpath("/tmp")                    # macOS: "/private/tmp"
p File.realpath(".") == File.realpath(Dir.pwd)    # true
begin
  File.realpath("/no/such/path/xyz")
  puts "no error"
rescue Errno::ENOENT
  puts "ENOENT"                            # must exist
end
