# Lazy $~ scoping (save_match_scope_on_write / scoped_last_match):
# the caller-save now happens at the first last_match WRITE inside a
# method scope and reads gate through the innermost method frame's
# saved-marker (callee that never matches must read nil, not the
# caller's match). This matrix pins the frame-local contract across
# nested methods, blocks sharing the method scope, match failure
# writes, gsub-block, and the callee-read-isolation case.
# frame-local $~ semantics matrix
def inner; "zzz" =~ /z+/; end
"abc" =~ /b/
inner
p $~[0]              # caller $~ survives nested method's match
def m2
  "hello" =~ /e(l+)/
  [$1, $~[0]]
end
p m2
p $~[0]              # still "b"
# block shares method scope
def m3
  [1].each { "xy" =~ /y/ }
  $~ && $~[0]
end
p m3                 # block's match visible in method
p $~[0]              # caller unaffected
# match FAILURE also scoped (writes nil)
def m4; "a" =~ /q/; $~.nil?; end
p m4
p $~[0]              # caller's still intact
# NOTE: scan(no-block) not setting $~ is a pre-existing gap
# (fails identically on the eager implementation) — not pinned.
def m6; "abc".gsub(/b/) { |m| m.upcase }; $~ && $~[0]; end
p m6
p $~[0]
# toplevel write then read in sub-method without match
def m7; $~ ? $~[0] : "nil-in-callee"; end
p m7                 # CRuby: $~ is METHOD-LOCAL — callee sees its own (nil)
