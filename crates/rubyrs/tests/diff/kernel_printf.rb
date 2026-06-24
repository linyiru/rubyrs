# Kernel#printf — print(format(...)) to $stdout (and the printf(io, ...)
# form to an IO). rubyrs had format/sprintf but not printf.
printf("%d + %d = %d\n", 2, 3, 5)
printf("%-6s|%6.2f|\n", "hi", 3.14159)
printf("%05d %x %o\n", 42, 255, 8)
printf("no args\n")
require "stringio"
io = StringIO.new
printf(io, "to io: %s=%d\n", "x", 7)
print io.string
p printf("")
