# File/Dir I/O failures raise the matching Errno::* (a SystemCallError
# subclass), so `rescue Errno::ENOENT` catches them. Regression: every
# failure was raised as a plain RuntimeError, which the pervasive
# `rescue Errno::ENOENT` idiom silently missed.
MISSING = "/no/such/rubyrs_path_zzz".freeze

begin
  File.read(MISSING)
rescue Errno::ENOENT => e
  puts "read: #{e.class} sce=#{e.is_a?(SystemCallError)}"
end

begin
  File.size(MISSING)
rescue Errno::ENOENT => e
  puts "size: #{e.class}"
end

begin
  Dir.entries(MISSING)
rescue Errno::ENOENT => e
  puts "entries: #{e.class}"
end

begin
  Dir.chdir(MISSING)
rescue Errno::ENOENT => e
  puts "chdir: #{e.class}"
end

# A bare rescue (StandardError) also catches it.
begin
  File.read(MISSING)
rescue => e
  puts "bare: #{e.class}"
end
