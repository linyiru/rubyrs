# File class-method shims — read / write / exist? / size /
# basename / dirname / extname. Uses /tmp/ paths (the macOS +
# Linux test runners both have it; CI matches).

base = "/tmp/rubyrs_file_io_test.txt"
File.write(base, "hello\nworld\n")

# Read back the content.
p File.read(base)

# Existence + size.
p File.exist?(base)
p File.exist?("/no/such/path/xyz123")
p File.size(base)

# Basename / dirname / extname.
p File.basename(base)
p File.dirname(base) == "/tmp"
p File.extname(base)

# Overwrite + read.
File.write(base, "replaced\n")
p File.read(base)

# Write with non-string body — coerced via to_s.
File.write(base, 42)
p File.read(base)

# Empty content.
File.write(base, "")
p File.read(base)
p File.size(base)

# Reading a missing file raises. Match on the StandardError
# ancestor so both rubyrs RuntimeError and CRuby's
# Errno::ENOENT-derived class catch.
begin
  File.read("/no/such/file/abc")
  puts "did NOT raise"
rescue StandardError
  puts "raised as expected"
end

# Cleanup.
File.write(base, "")
