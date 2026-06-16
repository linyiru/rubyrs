# `String#encode!` — in-place transcode/encoding-set, returns self
# (built on `#encode` + `#replace`). Surfaced by bridgetown-core's
# `ERBView#initialize` (`@buffer.encode!`).
s = String.new("hello")
r = s.encode!
p r.equal?(s)
p s
p "world".encode!("UTF-8")
