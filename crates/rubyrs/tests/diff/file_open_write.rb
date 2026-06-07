# Write-mode File.open (w / a) buffers writes (puts/write/<</print) and
# flushes on close; #readline returns the first line. Round-trips a
# /tmp file and cleans up.
path = "/tmp/rubyrs_diff_fopen.txt"
File.open(path, "wb") do |f|
  f.puts "line one"
  f.write "raw"
  f << " chained\n"
  f.print "tail"
end
p File.read(path)
File.open(path, "a") { |f| f.puts "\nappended" }
p File.read(path)
p File.open(path, "rb", &:readline)
