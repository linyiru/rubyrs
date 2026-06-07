# File.fnmatch? / fnmatch — glob-style matching with the FNM_* flags.
p File.fnmatch?("*.rb", "foo.rb")
p File.fnmatch?("*.rb", "foo.txt")
p File.fnmatch?("*", "foo/bar")
p File.fnmatch?("foo/*", "foo/bar")
p File.fnmatch?("?at", "cat")
p File.fnmatch?("[a-c]x", "bx")
p File.fnmatch?("[!a-c]x", "dx")
p File.fnmatch?("*", ".hidden")             # leading dot not matched by *
p File.fnmatch?(".*", ".hidden")
p File.fnmatch?("\\*", "*")                  # escaped literal
p File.fnmatch?("a[b", "a[b")               # unterminated class → false
p File.fnmatch?("*", "foo/bar", File::FNM_PATHNAME)
p File.fnmatch?("FOO*", "foobar", File::FNM_CASEFOLD)
p [File::FNM_PATHNAME, File::FNM_DOTMATCH, File::FNM_CASEFOLD]
