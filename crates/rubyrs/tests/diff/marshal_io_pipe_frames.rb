# Marshal dump-to-port / load-from-port over an in-process IO.pipe —
# the protocol the parallel gem's work_in_processes runs across a fork
# boundary (rubocop --parallel). BEHAVIOR comparison, not wire bytes:
# rubyrs writes length-framed \x04\x08 payloads, CRuby its raw stream;
# both must round-trip identically and keep CRuby's port discipline
# (dump returns the port; sequential frames on one pipe; EOFError at
# stream end; TypeError for un-dumpable graphs and non-IO ports).
r, w = IO.pipe

# dump(obj, port) returns the port
p Marshal.dump([1, :two], w).equal?(w)

# multiple sequential frames on one pipe round-trip in order
Marshal.dump({ "k" => [3.5, nil, true] }, w)
Marshal.dump("third", w)
p Marshal.load(r)
p Marshal.load(r)
p Marshal.load(r)

# 3-arg form (port + depth limit)
Marshal.dump([:deep, [1]], w, -1)
p Marshal.load(r)

# worker-loop shape: Integer job indices then a result array
3.times { |i| Marshal.dump(i, w) }
3.times { p Marshal.load(r) }

# an un-dumpable graph raises TypeError BEFORE writing a frame, so the
# stream stays consistent for the next real frame
begin
  Marshal.dump(proc { 1 }, w)
rescue TypeError => e
  p [e.class.name, e.message]
end
Marshal.dump(:after_error, w)
p Marshal.load(r)

# Integer in the port slot is a depth limit, not a port
p Marshal.dump([1], 4).class

# a non-IO port raises TypeError
begin
  Marshal.dump([1], Object.new)
rescue TypeError => e
  p e.class.name
end

# EOF discipline: writer closed + drained pipe -> EOFError
w.close
p r.eof?
begin
  Marshal.load(r)
rescue EOFError => e
  p [e.class.name, e.message]
end
r.close

# any object with #read/#write works as a port (StringIO)
require "stringio"
sio = StringIO.new
Marshal.dump({ nested: { a: 1 } }, sio)
Marshal.dump([:frame2], sio)
sio.rewind
p Marshal.load(sio)
p Marshal.load(sio)
