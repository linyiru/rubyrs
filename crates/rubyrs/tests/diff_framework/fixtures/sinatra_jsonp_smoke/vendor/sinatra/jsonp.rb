# Vendored verbatim from sinatra-jsonp-0.5.0
# (https://rubygems.org/gems/sinatra-jsonp, MIT license).
# Source: lib/sinatra/jsonp.rb in the gem tarball.
# Loaded with NO modification on either runtime — the harness
# proves the same authoring shape works on rubyrs (via the
# vendored micro-Sinatra + multi_json shim) and CRuby (via the
# real sinatra gem + real multi_json gem). Only mechanical
# change: `require "sinatra/base"` is replaced by the
# runtime-aware bootstrap in compat.rb so the file can be
# loaded from a non-LOAD_PATH location.
require 'multi_json'

module Sinatra
  module Jsonp
    def jsonp(*args)
      if args.size > 0
        data = MultiJson.dump args[0], :pretty => settings.respond_to?(:json_pretty) && settings.json_pretty
        if args.size > 1
          callback = args[1].to_s
        else
          ['callback','jscallback','jsonp','jsoncallback'].each do |x|
            callback = params.delete(x) unless callback
          end
        end
        if callback
          callback.tr!('^a-zA-Z0-9_$\.', '')
          content_type :js
          response = "#{callback}(#{data})"
        else
          content_type :json
          response = data
        end
        response
      end
    end
    alias JSONP jsonp
  end
  helpers Jsonp
end
