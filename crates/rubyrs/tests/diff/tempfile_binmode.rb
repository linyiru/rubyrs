# Tempfile#binmode? reflects whether `binmode` was explicitly called, NOT
# the open encoding — CRuby: a tempfile opened with `encoding:` is not in
# binmode until `binmode` is requested. rack's Rack::Multipart::UploadedFile
# reports `binmode?` to say whether an upload was opened in binary mode.
require 'tempfile'

t = Tempfile.new(["a", ".txt"], encoding: Encoding::BINARY)
p t.binmode?          # false — encoding alone doesn't imply binmode

t2 = Tempfile.new(["b", ".txt"], encoding: Encoding::BINARY)
t2.binmode
p t2.binmode?         # true — explicit binmode

t3 = Tempfile.new("c")
p t3.binmode?         # false
