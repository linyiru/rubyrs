# `sinatra-meta-info` — third-party-style Sinatra plugin written
# in the canonical real-gem authoring pattern. The shape is what
# `sinatra-cors`, `sinatra-flash`, `sinatra-respond_to`, and
# friends all use: a module under the `Sinatra` namespace
# providing `self.registered(app)`, plus a nested `Helpers`
# module that gets mixed in via `app.helpers Helpers`.
#
# The plugin's effect surface (observable in route output for
# byte-diff):
#   * Adds a `meta_info_string` helper available in every route
#     block, returning a deterministic string built from
#     @meta_info_ivar (seeded by the before filter).
#   * Adds a `meta_info_request_count` helper that reads the
#     @meta_info_count ivar (also seeded by before).
#   * Installs a `before` filter that seeds those ivars on
#     every dispatch.
#   * Adds a `/__meta` route directly via the plugin (real
#     plugins do this to expose introspection or diagnostic
#     endpoints).
#   * Exposes a constant SinatraMetaInfo::VERSION for app
#     code to read.

module Sinatra
  module MetaInfo
    VERSION = "0.2.0".freeze

    # The standard `helpers Module` argument — methods here
    # become instance methods of the Sinatra::Base subclass
    # via `app.helpers Helpers`, and route blocks (which run
    # via instance_exec on a per-request dispatch instance)
    # can call them without an explicit receiver.
    module Helpers
      def meta_info_string
        "[plugin v#{Sinatra::MetaInfo::VERSION}] " \
          "seeded=#{@meta_info_seeded}"
      end

      def meta_info_request_count
        @meta_info_count
      end
    end

    # Entry point invoked by `Sinatra::Base.register Sinatra::MetaInfo`.
    # Receives the host app class (a Sinatra::Base subclass) and
    # wires every plugin contribution onto it. The order
    # mirrors what real plugins do: helpers FIRST (so before /
    # routes can use them), THEN filters, THEN routes.
    def self.registered(app)
      app.helpers Helpers

      app.before do
        @meta_info_seeded = true
        @meta_info_count = 1
      end

      app.get "/__meta" do
        # Direct call to a Helpers method from a plugin-
        # installed route — exercises the same instance_exec
        # dispatch path the app's own routes do.
        meta_info_string
      end
    end
  end
end
