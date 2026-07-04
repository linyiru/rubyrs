# `Dir.children` / `Dir.entries` with the optional `encoding` keyword
# argument — accepted on both (rubyrs: ignored; strings are
# UTF-8, which is also CRuby's default here). Surfaced by the real stdlib
# fileutils.rb, whose `Entry_#entries` calls
# `Dir.children(path, encoding: path.encoding)` from every recursive
# traversal — without kwarg acceptance `FileUtils.rm_rf` raised
# NoMethodError mid-walk, swallowed it (force), and silently removed
# NOTHING (bridgetown-core's Marshal cache then went stale).
base = "/tmp/rubyrs_children_enc_fixture"
system("rm", "-rf", base)
Dir.mkdir(base)
File.write(File.join(base, "b.txt"), "")
File.write(File.join(base, "a.txt"), "")
Dir.mkdir(File.join(base, ".hidden"))

p Dir.children(base, encoding: Encoding::UTF_8).sort
p Dir.children(base, encoding: "UTF-8").sort
p Dir.entries(base, encoding: Encoding::UTF_8).sort

# the fileutils shape: **opts built from a path's own encoding
opts = { encoding: base.encoding }
p Dir.children(base, **opts).sort

system("rm", "-rf", base)
