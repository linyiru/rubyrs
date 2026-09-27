# Assignment-syntax calls (`recv.x = v`, `recv[k] = v`) evaluate to the RHS,
# never the setter's return. The interpreter implements that by stamping
# `swap_return` on the callee's frame. Under tier 2 the setter can run as a
# frame-lite body whose nested lite->lite chain materializes several frames
# at once; the RHS swap must land on the SETTER's frame, not the innermost
# materialized one (which made a zero-arg self-call inside the setter
# return the setter's argument).

class R
  attr_reader :out, :log

  def initialize(env)
    @env = env
    @log = []
  end

  def get_header(k) = @env[k]
  def routes = get_header("routes")
  def fetch2(a, b) = get_header(a) || get_header(b)
  def chain = routes

  def es=(x)
    p routes
  end

  def es2=(x)
    y = routes
    @out = [y, x]
  end

  def es3=(x)
    @out = fetch2("missing", "routes")
  end

  def es4=(x)
    @out = chain
  end

  def []=(k, v)
    @out = [k, routes, v]
  end

  def blk=(x)
    @out = [1].map { routes }
  end

  def ret_routes=(x)
    routes
  end
end

class W
  attr_writer :name
  def initialize = (@name = nil)
  def name_via = @name
end

class NewInit
  attr_reader :got
  def initialize(env)
    @env = env
    @got = routes
  end
  def get_header(k) = @env[k]
  def routes = get_header("routes")
end

r = R.new({ "routes" => :RT })
w = W.new
4.times do |i|
  r.es = "arg"
  r.es2 = "arg2"
  p r.out
  r.es3 = "arg3"
  p r.out
  r.es4 = "arg4"
  p r.out
  r[:k] = "v"
  p r.out
  r.blk = "b"
  p r.out
  p(r.ret_routes = "rhs")
  p r.send(:ret_routes=, "rhs")
  p(r.es2 = "val#{i}")
  p(w.name = "n#{i}")
  p w.name_via
  p NewInit.new({ "routes" => :NI }).got
end
