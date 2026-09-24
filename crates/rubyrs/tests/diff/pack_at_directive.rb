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

# A count no allocator can satisfy is NoMemoryError, not a VM abort.
%w[@9000000000000000000 x9000000000000000000 a9000000000000000000].each do |f|
  begin
    [""].pack(f)
    p :no_error
  rescue NoMemoryError => e
    p [f[0], e.class, e.message]
  end
end

# A value-consuming directive wants more values than remain: CRuby
# raises instead of packing defaults, which also bounds the output.
[["C1000000000", [1]], ["n", []], ["aC", ["a"]], ["U3", [65, 66]], ["m", []]].each do |f, vals|
  begin
    vals.pack(f)
    p :no_error
  rescue ArgumentError => e
    p [f, e.class, e.message]
  end
end
