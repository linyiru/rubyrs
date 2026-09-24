# The `@n` pack/unpack directive: absolute byte offset (pack pads
# with NUL or truncates; unpack jumps).
p [1, 2].pack("C@4C")
p [1, 2, 3].pack("C3@1")
p [7].pack("@2C")
p "abcdef".unpack("@2C2")
p "abcdef".unpack("C@0C")
p "abcdef".unpack("@6C")

# ActiveSupport::Cache::Coder's header template shape.
signature = "\x00\x11".b
template = "@#{signature.bytesize}C"
p (signature + "\x05".b).unpack(template)

begin
  "abc".unpack("@9C")
rescue ArgumentError => e
  p e.class
end
