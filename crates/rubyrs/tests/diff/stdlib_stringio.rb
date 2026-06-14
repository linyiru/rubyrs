# StringIO vendored as Tier 3 pure-Ruby stdlib (subset).
# Buffer-only — no fd, no mode flags. Covers the canonical
# gem-helper pattern of "build a string with IO-shaped writes,
# then read it back". Fixture runs under `--features stdlib`
# only (registered as `#[cfg(feature = "stdlib")]`).

require 'stringio'

# Class identity.
puts StringIO.class.name           # "Class"
puts StringIO.new.class.name       # "StringIO"

# Empty / initial-content construction.
empty = StringIO.new
puts empty.string.inspect          # ""
puts empty.size                    # 0
puts empty.eof?                    # true

seeded = StringIO.new("hello")
puts seeded.string                 # "hello"
puts seeded.size                   # 5
puts seeded.length                 # 5 (alias)

# write / << / puts / print build the buffer; `<<` chains.
io = StringIO.new
io << "a" << "b" << "c"
puts io.string                     # "abc"
io.write("d", "e")
puts io.string                     # "abcde"
io.puts "line"                     # adds "line\n"
puts io.string                     # "abcdeline\n"
io.print "no", "newline"           # adds "nonewline"
puts io.string                     # "abcdelinenonewline" with trailing \n in the middle
puts io.string.inspect             # "\"abcdeline\\nnonewline\""

# puts with no args appends a single newline.
io2 = StringIO.new
io2.puts
puts io2.string.inspect            # "\"\\n\""

# puts skips newline if the arg already ends with one (CRuby).
io3 = StringIO.new
io3.puts "x\n"
io3.puts "y"
puts io3.string.inspect            # "\"x\\ny\\n\""

# pos / rewind / seek / read.
r = StringIO.new("hello world")
puts r.pos                         # 0
puts r.read(5)                     # "hello"
puts r.pos                         # 5
puts r.read                        # " world" (whole rest)
puts r.pos                         # 11
puts r.eof?                        # true
puts r.read(1).inspect             # nil at EOF
r.rewind
puts r.pos                         # 0
puts r.read(5)                     # "hello"
r.seek(6, 0)                       # SEEK_SET absolute
puts r.read                        # "world"
r.seek(-5, 2)                      # SEEK_END relative
puts r.read                        # "world"

# gets reads up to and including newline.
g = StringIO.new("alpha\nbeta\ngamma")
puts g.gets.inspect                # "alpha\n"
puts g.gets.inspect                # "beta\n"
puts g.gets.inspect                # "gamma" (no trailing newline)
puts g.gets.inspect                # nil at EOF

# each_line / each block iteration.
each_io = StringIO.new("one\ntwo\nthree\n")
collected = []
each_io.each_line { |l| collected << l }
puts collected.inspect             # ["one\n", "two\n", "three\n"]

# Block-form StringIO.open auto-closes.
result = StringIO.open("seeded-content") do |sio|
  sio.read
end
puts result.inspect                # "seeded-content"

# close + closed? — a no-op marker; subsequent ops still work
# in the Tier 3 model (real CRuby raises IOError but the
# buffer-only shape doesn't need that yet).
c = StringIO.new
puts c.closed?                     # false
c.close
puts c.closed?                     # true

# binmode — no-op returning self (StringIO is always byte-oriented).
# rack's RewindableInput / Multipart / Lint call `io.binmode`. CRuby
# has `binmode` but NOT `binmode?`, so respond_to? must mirror that.
b = StringIO.new("data")
puts b.binmode.equal?(b)           # true (returns self)
puts b.read                        # "data" (still usable after binmode)
puts b.respond_to?(:binmode)       # true
puts b.respond_to?(:binmode?)      # false
