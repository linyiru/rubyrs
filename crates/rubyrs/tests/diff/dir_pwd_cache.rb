# Dir.pwd / File.expand_path use a cached getcwd; Dir.chdir must
# invalidate it. Machine-independent checks (uses "/" for absolutes).
p Dir.pwd == Dir.pwd                       # cache stable across calls
p File.expand_path("x").end_with?("/x")    # relative resolves against cwd
p File.expand_path("a/b").end_with?("/a/b")
base = Dir.pwd
Dir.chdir("/") do
  p Dir.pwd                                 # "/" — cache invalidated on chdir
  p File.expand_path("z")                   # "/z"
  p File.expand_path(".")                   # "/"
end
p Dir.pwd == base                           # restored + invalidated again
p File.expand_path(".") == base             # expand_path cwd consistent with pwd
