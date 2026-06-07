# Pathname#ascend yields self then each ancestor, stripping a trailing
# component per step (block form returns nil).
require "pathname"
acc = []
Pathname.new("/path/to/some/file.rb").ascend { |pn| acc << pn.to_s }
p acc
rel = []
Pathname.new("rel/a/b").ascend { |pn| rel << pn.to_s }
p rel
p Pathname.new("/").ascend { |pn| }   # block form returns nil
root = []
Pathname.new("/").ascend { |pn| root << pn.to_s }
p root
