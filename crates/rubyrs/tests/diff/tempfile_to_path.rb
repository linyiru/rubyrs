# Tempfile responds to `to_path` (returning its path), so path-coercing
# File APIs accept it directly — rack's spec_multipart does
# `File.extname(env["rack.tempfiles"][0])` on a Tempfile.
require 'tempfile'

t = Tempfile.new(["report", ".tar.gz"])
p t.respond_to?(:to_path)        # true
p t.to_path == t.path            # true
p File.extname(t)                # ".gz"
p File.basename(t).end_with?(".tar.gz")   # true
