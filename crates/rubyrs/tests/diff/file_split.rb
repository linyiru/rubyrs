# File.split(path) == [File.dirname(path), File.basename(path)].
p File.split("/a/b/c.rb")
p File.split("foo.rb")
p File.split("/")
p File.split("/usr/")
p File.split("")
p File.split("a/b/")
p File.split("./rel.rb")
p File.split("/single")
