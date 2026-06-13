# eval(src, binding) must preserve the source's line numbers: when the
# binding captures locals, the implementation wraps the source in a
# lambda so prism resolves those locals — that wrap must NOT shift line
# numbers (it goes on the SAME line as the source's first line). rack's
# Builder.parse_file checks `__LINE__` through `eval(script, binding,
# path)`. A leading BOM is stripped (CRuby eval ignores one).

def make_b
  marker = "M"
  binding
end
b = make_b

# __LINE__ inside the eval'd string is line-1-relative to the string.
puts eval("__LINE__", b)                      # 1
puts eval("x = 1\n__LINE__", b)               # 2
puts eval("a = 1\nb = 2\nc = 3\n__LINE__", b) # 4
# captured local still resolves alongside __LINE__ tracking.
puts eval("marker + __LINE__.to_s", b)        # "M1"

# A leading UTF-8 BOM is ignored, and line numbers still start at 1.
puts eval("﻿__LINE__", b)                # 1
puts eval("﻿marker", b)                  # "M"

# Multi-statement with the captured local on a later line.
src = "p1 = 10\np2 = 20\n[marker, p1 + p2, __LINE__]"
p eval(src, b)                                # ["M", 30, 3]
