# eval/class_eval of a non-UTF-8 source: the produced string literals
# carry the source's encoding (Tilt evals templates written in the
# template's own encoding). US-ASCII just re-tags; Shift_JIS transcodes.
class C; end

# US-ASCII source -> literal tagged US-ASCII
asrc = "def m; 'plain'; end".dup.force_encoding('US-ASCII')
C.class_eval(asrc, "a.rb", 1)
v = C.new.m
p v
p v.encoding.to_s

# Shift_JIS source -> literal in Shift_JIS, transcodes back to UTF-8
code = "def jp; \"#{"ふが"}\"; end".encode('Shift_JIS')
class D; end
D.class_eval(code, "jp.rb", 1)
r = D.new.jp
p r.encoding.to_s
p r.bytes
p r.encode('UTF-8')
