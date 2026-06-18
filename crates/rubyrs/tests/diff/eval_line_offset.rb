# eval(src, binding, file, line) / class_eval(src, file, line) — the line
# arg sets where the source's first line maps to in backtraces, so a
# template engine's compiled-method exceptions report template lines.
# (Compares only the file:line prefix; the `in '<method>'` label format
# is checked elsewhere.)
def fileline(frame); frame.split(":in ").first; end
def boom_line(src, line)
  eval(src, binding, "tmpl.rb", line)
rescue => e
  fileline(e.backtrace.first)
end
p boom_line("raise 'x'", 10)
p boom_line("a = 1\nb = 2\nraise 'y'", 10)
p boom_line("\n\n\n\n\nraise 'z'", -4)

class C; end
def ce_line
  C.class_eval("def m\n  raise 'boom'\nend", "ce.rb", 50)
  C.new.m
rescue => e
  fileline(e.backtrace.first)
end
p ce_line
