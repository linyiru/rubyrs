# E3 IO coding surface (core): File.read's coding option stamps a
# TAG on the raw bytes (no transcoding for single names — an
# invalid-in-encoding read keeps its bytes and reports
# valid_encoding? false); a missing option follows
# Encoding.default_external, whose setter is the strict side
# (ArgumentError on unknown names — the read path only warns).
path = "/tmp/rubyrs-e3-core-#{Process.pid}.bin"
File.binwrite(path, "caf\xE9\n".b)

s = File.read(path)
p [s.bytes, s.encoding.name, s.valid_encoding?]
b = File.read(path, encoding: "ASCII-8BIT")
p [b.bytes, b.encoding.name, b.valid_encoding?]
a = File.read(path, encoding: "US-ASCII")
p [a.encoding.name, a.valid_encoding?]

Encoding.default_external = "ASCII-8BIT"
p File.read(path).encoding.name
Encoding.default_external = Encoding::UTF_8
p File.read(path).encoding.name
p Encoding.default_external.name

begin
  Encoding.default_external = "NOPE"
rescue ArgumentError => e
  puts "setter: #{e.message}"
end

# Write side stays byte-verbatim regardless of the string's tag.
File.write(path, "caf\xE9".dup.force_encoding("ASCII-8BIT"))
p File.binread(path).bytes
File.delete(path)
