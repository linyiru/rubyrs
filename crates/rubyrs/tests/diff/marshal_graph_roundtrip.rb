# Marshal object-graph coverage through the byte stream, exercised the
# way rubocop's parallel results cross a worker pipe: link-table shared
# references (one Source::Buffer shared by many Ranges), cycles, user
# objects with nested class names, marshal_dump hooks that DROP an
# un-dumpable ivar (Offense drops @corrector, whose graph holds Procs),
# Range / Bignum / encodings / frozen-flag behavior. Round-trips go
# through an IO.pipe so the byte path is what's tested (the same-process
# registry token could otherwise satisfy string-form dump/load for
# out-of-subset shapes).
R, W = IO.pipe

def rt(obj)
  Marshal.dump(obj, W)
  Marshal.load(R)
end

# --- link table: shared references serialize once, load as one object
s = +"shared source text"
pair = rt([s, s])
p pair[0].equal?(pair[1])
p pair[0]

arr = [1, 2]
h = { "x" => arr }
g = rt([arr, h, arr])
p g[0].equal?(g[2])
p g[0].equal?(g[1]["x"])

# --- cycles: self-referential array / hash / object ivars
a = []
a << a
back = rt(a)
p back[0].equal?(back)

module MGraph
  class Node
    attr_accessor :label, :peer
    def initialize(label)
      @label = label
    end
  end
end
n1 = MGraph::Node.new("n1")
n2 = MGraph::Node.new("n2")
n1.peer = n2
n2.peer = n1
m1 = rt(n1)
p [m1.class.name, m1.label, m1.peer.label, m1.peer.peer.equal?(m1)]

# --- buffer-sharing shape (what Offense graphs actually look like)
class MiniBuffer
  attr_reader :name, :source
  def initialize(name, source)
    @name = name
    @source = source
  end
end
class MiniRange
  attr_reader :buffer, :b, :e
  def initialize(buffer, b, e)
    @buffer = buffer
    @b = b
    @e = e
  end
end
buf = MiniBuffer.new("lib/x.rb", "def a\n  1\nend\n")
offs = [MiniRange.new(buf, 0, 5), MiniRange.new(buf, 8, 9), MiniRange.new(buf, 10, 13)]
lo = rt([offs, true])
ranges, ok = lo
p ok
p ranges.map { |r| [r.b, r.e] }
p ranges[0].buffer.equal?(ranges[1].buffer)
p ranges[1].buffer.equal?(ranges[2].buffer)
p ranges[0].buffer.source

# --- marshal_dump hook drops an un-dumpable ivar (Offense/@corrector)
class HookedResult
  attr_reader :message, :status
  def initialize(message, status, corrector)
    @message = message
    @status = status
    @corrector = corrector # holds a Proc -> raw-ivar dump would die
  end
  def marshal_dump
    [@message, @status]
  end
  def marshal_load(a)
    @message, @status = a
  end
end
hr = rt(HookedResult.new("Line too long.", :uncorrected, proc { :fix }))
p [hr.class.name, hr.message, hr.status, hr.instance_variable_defined?(:@corrector)]

# --- Range: inclusive / exclusive / non-Int endpoints / shared in graph
p rt(1..9)
p rt(2...5)
p rt("aa".."ad")
rr = 3..7
two = rt([rr, rr])
p two[0].equal?(two[1])
p rt([1..2, [3...4]])

# --- Bignum both signs; boundary stays Integer-identical
p rt(10**30)
p rt(-(2**100))
p rt(2**62 + 1)
p rt([10**25, 10**25].inject(:+) == 2 * 10**25)

# --- Float specials
p rt([1.5, -0.0, Float::INFINITY, -Float::INFINITY, 1.0e300])

# --- encodings survive; frozen does NOT (CRuby loads unfrozen)
u = rt("héllo")
p [u, u.encoding.name]
b = rt("\xFF\x00ab".b)
p [b.bytes, b.encoding.name]
fz = rt("frozen str".freeze)
p [fz, fz.frozen?]

# --- exception graph (parallel's ExceptionWrapper path)
err = RuntimeError.new("boom")
err.set_backtrace(["x.rb:1:in 'go'", "x.rb:9"])
e2 = rt(err)
p [e2.class.name, e2.message, e2.backtrace]

# --- Struct instance over the pipe (S tag)
SPoint = Struct.new(:x, :y)
sp = rt(SPoint.new(3, [4, 5]))
p [sp.class.name, sp.x, sp.y]

# --- symbols intern back to the same symbol
sym = rt([:sev, :sev, :other])
p [sym[0].equal?(sym[1]), sym]

W.close
R.close
