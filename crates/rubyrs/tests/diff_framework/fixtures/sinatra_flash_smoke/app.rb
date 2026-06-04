# sinatra_flash_smoke app — real sinatra-flash-0.3.0 vendored 1:1
# under vendor/sinatra/. Exercises the FlashHash class API
# (write-to-@next / read-from-@now / sweep / keep / discard) and
# the styled_flash HTML helper in single-request routes. The
# diff_framework doesn't thread cookies across scenarios; the
# cross-request session round-trip itself is verified separately
# by sinatra_lite's Rack::Session::Cookie shim.

require_relative "sinatra_compat"

class SinatraFlashSmokeApp < Sinatra::Base
  # Modular form needs the explicit register call — real Sinatra's
  # `Sinatra.register Flash` at the gem's last line installs onto
  # the classic Application class, not every Sinatra::Base
  # subclass. rubyrs's sinatra_lite forwards module-level helpers
  # onto Sinatra::Base so the helpers are technically already
  # available without this line, but having it keeps both
  # runtimes' parity identical.
  register Sinatra::Flash

  use Rack::Session::Cookie

  # Exercise the FlashHash @now/@next dance in one request.
  get "/api/sweep_cycle" do
    f = flash
    f[:notice] = "hello"
    f[:warning] = "watch out"
    sweep_result = f.sweep
    "now=#{sweep_result.inspect} next_empty=#{f.next.empty?}"
  end

  # `keep` carries a key forward into @next.
  get "/api/keep_one" do
    f = flash
    f[:a] = "alpha"
    f[:b] = "beta"
    f.sweep
    f.keep(:a)
    "kept_next=#{f.next.inspect}"
  end

  # `discard` clears @next.
  get "/api/discard" do
    f = flash
    f[:x] = 1
    f[:y] = 2
    f.discard
    "after_discard=#{f.next.inspect}"
  end

  # `styled_flash` — empty flash returns empty string.
  get "/api/styled_empty" do
    styled_flash
  end

  # `styled_flash` — renders <div id='flash'> wrapping each
  # message in <div class='flash <type>'>.
  get "/api/styled_messages" do
    f = flash
    f[:notice] = "Saved!"
    f[:warning] = "Be careful"
    f.sweep
    styled_flash
  end

  # `flash(:key)` returns a separate FlashHash; styled HTML
  # has the suffixed id.
  get "/api/styled_keyed" do
    f = flash(:login)
    f[:info] = "Login successful"
    f.sweep
    styled_flash(:login)
  end

  get "/" do
    "backend: #{SERVER_BACKEND}"
  end
end

HARNESS_RUN_APP.call(SinatraFlashSmokeApp)
