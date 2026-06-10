# File.delete / File.unlink: removes each named file, returns the
# count removed; a missing file raises Errno::ENOENT (left-to-right,
# so files before the failing one stay deleted). Added alongside the
# bom|utf-8 work — the BOM fixture wanted cleanup and the subset had
# no delete at all.
A = "/tmp/rubyrs_file_delete_a.txt"
B = "/tmp/rubyrs_file_delete_b.txt"
C = "/tmp/rubyrs_file_delete_c.txt"

def stage(*paths)
  paths.each { |p| File.open(p, "wb") { |f| f.write("x") } }
end

stage(A)
puts "single: #{File.delete(A)}"
puts "gone:   #{File.exist?(A)}"

stage(A, B)
puts "multi:  #{File.delete(A, B)}"
puts "gone:   #{File.exist?(A)} #{File.exist?(B)}"

stage(A)
puts "unlink: #{File.unlink(A)}"

begin
  File.delete(C)
rescue Errno::ENOENT => e
  puts "enoent: #{e.class}"
end

# Left-to-right partial processing: A is deleted before C raises.
stage(A)
begin
  File.delete(A, C)
rescue Errno::ENOENT
  puts "partial: a_gone=#{!File.exist?(A)}"
end
