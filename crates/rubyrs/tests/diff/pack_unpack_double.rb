# Array#pack / String#unpack(1) IEEE float directives: D/d (native double),
# E (LE double), G (BE double), F/f (native single), e (LE single), g (BE
# single). prism's `Serialize#load_double` reads `unpack1("D")` for every
# Float literal in parsed source — without these, RuboCop's parser_prism
# engine returns ZERO offenses for any file containing a Float.

# round-trip each directive (native ones are host-endian but pack+unpack
# on the same host cancels out, so the value is stable cross-platform)
[1.5, 3.14159, -0.001, 1e10, 0.0, -42.5].each do |v|
  %w[D d E G].each { |dir| puts "#{dir} #{v} -> #{[v].pack(dir).unpack1(dir)}" }
end

# single precision (lossy → compare the round-tripped value, same on both)
[2.5, -1.25, 100.0].each do |v|
  %w[F f e g].each { |dir| puts "#{dir} #{v} -> #{[v].pack(dir).unpack1(dir)}" }
end

# Integer coerces to Float on pack
puts [42].pack("D").unpack1("D")          # 42.0

# multiple values + '*'
puts [1.0, 2.0, 3.0].pack("D*").unpack("D*").inspect   # [1.0, 2.0, 3.0]

# fixed byte widths
puts [1.5].pack("D").bytesize             # 8
puts [1.5].pack("F").bytesize             # 4

# explicit-endian byte layout is stable (LE double 1.5)
puts "\x00\x00\x00\x00\x00\x00\xF8\x3F".b.unpack1("E")   # 1.5
# BE double 1.5
puts "\x3F\xF8\x00\x00\x00\x00\x00\x00".b.unpack1("G")   # 1.5
