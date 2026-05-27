# Sinatra-shape DSL probe — two fixes locked here:
#
# (1) Block-form bare-call from subclass body resolves to a
#     parent class's singleton method. `class App < Base; get
#     '/' do ... end` is the canonical Sinatra route registrar
#     shape; before this fix, the block triggered do_call_block
#     instead of do_call, and the block-path didn't walk the
#     class's singleton chain — only the hardcoded primitive
#     whitelist. Result: NoMethodError on `get` even though Base
#     defined `def self.get(path, &block)`.
#
# (2) `Object#instance_exec(*args) { |*a| ... }` — like
#     instance_eval but the block receives the explicit args.
#     Sinatra's dispatch does `instance.instance_exec(&handler)`
#     to run a route block against a fresh request instance.

class Base
  @routes = {}
  class << self
    attr_reader :routes
    def get(path, &block)
      @routes ||= {}
      @routes[['GET', path]] = block
    end
    def post(path, &block)
      @routes ||= {}
      @routes[['POST', path]] = block
    end
    def dispatch(method, path, params = {})
      handler = @routes[[method, path]]
      return [404, "Not Found"] unless handler
      instance = new
      instance.instance_variable_set(:@params, params)
      body = instance.instance_exec(&handler)
      [200, body]
    end
  end

  def params; @params; end
  def h(s); s.to_s.gsub('<', '&lt;'); end
end

class App < Base
  get '/' do
    "Hello, World!"
  end

  get '/hello' do
    name = params[:name] || "anon"
    "Hello, #{h(name)}!"
  end

  post '/submit' do
    "Got: #{params[:body]}"
  end
end

# --- Drive the dispatch ---
puts App.dispatch('GET', '/').inspect
puts App.dispatch('GET', '/hello', name: "World").inspect
puts App.dispatch('GET', '/hello', name: "<script>").inspect
puts App.dispatch('POST', '/submit', body: "ping").inspect
puts App.dispatch('GET', '/missing').inspect

# --- instance_exec with args (the variadic shape) ---
class Ctx
  def initialize; @ivar = 42; end
end
c = Ctx.new
result = c.instance_exec(10, 20) { |x, y| [@ivar, x, y, x + y] }
puts result.inspect                          # [42, 10, 20, 30]

# --- instance_exec with no args ---
puts c.instance_exec { @ivar * 2 }           # 84

# --- Singleton inheritance ladder: grandparent → parent → child ---
class G
  def self.boom(x); "g-boom(#{x})"; end
end
class M < G; end
class L < M
  # bare call walks G via class_is_a's chain
  result = boom("ladder")
  puts "ladder bare: #{result}"
end
