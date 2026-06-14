# Writing to a pipe whose READ end has been closed raises Errno::EPIPE
# (CRuby semantics) — both for #write and #write_nonblock, and even with
# write_nonblock's `exception: false` (that flag only suppresses
# EAGAIN/EWOULDBLOCK, never EPIPE). rack's multipart "rejects insanely
# long boundaries" test relies on this to unblock the producer after
# Rack shuts the reader down. (The in-memory pipe shim can't model
# blocking/backpressure timing, but this terminal-write behaviour does
# match CRuby exactly.)

r, w = IO.pipe
r.close
begin
  w.write("hello")
  puts "write: no-error"
rescue Errno::EPIPE
  puts "write: EPIPE"
end

r2, w2 = IO.pipe
r2.close
begin
  w2.write_nonblock("hi", exception: false)
  puts "wnb: no-error"
rescue Errno::EPIPE
  puts "wnb: EPIPE"
end

# A normal drain-at-... write before any close still works.
r3, w3 = IO.pipe
n = w3.write("abc")
p n                      # 3
w3.close
p r3.read                # "abc"  (then EOF)
p r3.read(8)             # nil at EOF (writer closed)
