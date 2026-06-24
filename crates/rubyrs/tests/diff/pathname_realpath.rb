# Pathname#realpath / #realdirpath resolve `.`/`..` to an absolute path
# (realpath requires the file to exist). dry-validation resolves its
# bundled config/errors.yml via `Pathname(__FILE__).join(...).realpath`.
# (rubyrs approximates via File.expand_path — no symlink resolution.)
require "pathname"
base = Dir.pwd
p Pathname.new(".").realpath.to_s == base
p Pathname.new(base).realpath.to_s == base
p Pathname.new(base).realdirpath.to_s == base
p Pathname.new(base).realpath.class
begin
  Pathname.new(base + "/no_such_xyz123").realpath
rescue => e
  p e.class
end
