# Third-party-style Sinatra plugin — stands in for a real gem
# like `sinatra-flash` or `sinatra-cors`. Mirrors the older /
# simpler plugin authoring pattern that REOPENS Sinatra::Base
# directly (no `Sinatra.register` / `helpers` machinery required),
# which is the pattern most plugins originally used and which
# many existing gems still ship.
#
# What this fixture proves about M27:
#
#   * The reopened-class pattern works against a Sinatra::Base
#     loaded from a `require`d file (vendored micro-Sinatra on
#     rubyrs, real `sinatra/base` gem on CRuby).
#   * `define_method` inside an iterator block closes over the
#     loop-local `style` variable (M27 A4 batch). The captured
#     symbol is later used inside the resulting method body's
#     `case/when` — a real plugin shape that compiles into
#     a closure proto.
#   * Module-scoped frozen constants (`STYLES`) materialise
#     across the `require` boundary and are visible from inside
#     the captured block.
#
# The plugin only adds INSTANCE methods — no class-level routes,
# no before/after filters. That keeps it portable across both the
# vendored micro-Sinatra (which lacks filter inheritance from
# Sinatra::Base down to subclasses) and the real gem (which has
# more elaborate per-class filter machinery). The app is the one
# place that hooks routes; the plugin's role is to provide
# helpers usable from inside any app's route blocks.

module SinatraGreetPlugin
  VERSION = "0.1.0".freeze
  STYLES = [:formal, :casual, :friendly].freeze
end

class Sinatra::Base
  # Generate the three `greet_plugin_<style>` helpers via
  # define_method-with-block-capture. The outer `each` loops over
  # the styles list; for each style, define_method takes a block
  # that closes over `style`. When the resulting method is later
  # called against a dispatch instance via instance_exec, the
  # block's `style` resolves to the loop iteration's value — not
  # the last one. This is the M27 A4 contract.
  SinatraGreetPlugin::STYLES.each do |style|
    define_method("greet_plugin_#{style}") do |name|
      version = SinatraGreetPlugin::VERSION
      case style
      when :formal
        "Good day, #{name}, from greet-plugin v#{version}"
      when :casual
        "Hey #{name}! (greet-plugin v#{version})"
      when :friendly
        "Hello, dear #{name}! greet-plugin v#{version}"
      end
    end
  end

  # Plain reopened-Base instance method — no block-capture
  # involved, just a vanilla def. Sanity-pin that the bog-
  # standard plugin pattern (reopen + def) works too, alongside
  # the fancier define_method dance above.
  def greet_plugin_info
    "greet-plugin v#{SinatraGreetPlugin::VERSION} " \
      "styles=#{SinatraGreetPlugin::STYLES.map(&:to_s).join(',')}"
  end
end
