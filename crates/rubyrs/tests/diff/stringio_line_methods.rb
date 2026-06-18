require "stringio"
# readlines / readline / each_line(no block)→Enumerator
p StringIO.new("a\nb\nc\n").readlines
p StringIO.new("1\n2\n").readline
p StringIO.new("a\nb\nc\n").each_line.to_a
p StringIO.new("line\n").each_line.map(&:chomp)
# getc / each_char / each_byte
io = StringIO.new("abc"); p [io.getc, io.getc, io.getc, io.getc]
p StringIO.new("xyz").each_char.to_a
p StringIO.new("AB").each_byte.to_a
# printf
io2 = StringIO.new; io2.printf("%d-%s", 5, "x"); p io2.string
# ungetc
io3 = StringIO.new("xy"); io3.ungetc("z"); p io3.read
io4 = StringIO.new("hello"); io4.read(2); io4.ungetc("Q"); p io4.read
# readline raises at EOF
io5 = StringIO.new("only\n"); io5.readline
def t; yield; rescue => e; e.class; end
p t { io5.readline }
