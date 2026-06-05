# File.open / File.new / File instance read surface -- the
# pure-Ruby _io-veneer added to unblock the P3 Sinatra spike
# (logger 1.7's File.open(__FILE__) self-probe at module-load
# time). Assertions are boolean/count shaped (not echoing file
# bytes) so the rubyrs-vs-CRuby live diff stays robust.
#
# The fixture reads its OWN source via __FILE__ -- both runtimes
# run it from the same cwd with the same relative arg, so they
# read byte-identical content. Kept ASCII-only so line scanning
# is byte==char (avoids an unrelated multibyte index concern).

src = __FILE__

# Shape 1: block form yields a File, returns the block's value,
# and closes the handle afterwards.
returned = File.open(src) do |f|
  puts "is_file=#{f.is_a?(File)}"
  puts "path_matches=#{f.path == src}"
  :block_result
end
puts "block_return=#{returned.inspect}"

# Shape 2: gets advances; reading the rest reaches EOF.
File.open(src) do |f|
  line1 = f.gets
  puts "gets_is_string=#{line1.is_a?(String)}"
  puts "gets_has_newline=#{line1.end_with?("\n")}"
  rest = f.read
  puts "read_rest_is_string=#{rest.is_a?(String)}"
  puts "eof_after_full_read=#{f.eof?}"
  puts "gets_at_eof_nil=#{f.gets.nil?}"
end

# Shape 3: readlines returns every line; joined length equals
# the whole-file read length (consistency, not absolute count).
whole = File.open(src) { |f| f.read }
lines = File.open(src) { |f| f.readlines }
puts "readlines_is_array=#{lines.is_a?(Array)}"
puts "readlines_rejoins=#{lines.join == whole}"
puts "every_line_string=#{lines.all? { |l| l.is_a?(String) }}"

# Shape 4: each_line yields each line; count equals readlines.
count = 0
File.open(src) { |f| f.each_line { |_l| count += 1 } }
puts "each_line_count_matches=#{count == lines.length}"

# Shape 5: non-block open returns a File; close flips closed?.
g = File.open(src)
puts "noblock_is_file=#{g.is_a?(File)}"
puts "open_not_closed=#{g.closed? == false}"
g.close
puts "closed_after_close=#{g.closed?}"

# Shape 6: read with an explicit length returns up to N chars.
File.open(src) do |f|
  chunk = f.read(1)
  puts "read_n_len=#{chunk.length}"
end

# Shape 7: File.new with a String path behaves like open (no
# block) -- returns a readable File.
h = File.new(src)
puts "new_string_is_file=#{h.is_a?(File)}"
puts "new_string_readable=#{h.read.length > 0}"
h.close

# NOTE: File.new(<Integer fd>) and File#fileno are deliberate
# rubyrs DIVERGENCES (sandboxed runtime has no fd table -- both
# raise IOError) and are NOT parity-tested here: CRuby's
# File.new(bad_fd) raises Errno::EBADF and #fileno returns a real
# descriptor. The divergence is documented in preamble/file.rb
# and exercised by the logger load-time probe.
