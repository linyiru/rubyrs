# A lexically-scoped autoloaded constant must win over a same-named
# TOPLEVEL constant (CRuby lexical const lookup fires the nearer-scope
# autoload first). Surfaced by bridgetown's `register YAML` inside
# `module …FrontMatter::Loaders` — must bind `Loaders::YAML` (the
# autoloaded loader class), not the stdlib `::YAML` module.
target = "/tmp/rubyrs_lex_autoload.rb"
File.write(target, <<~RUBY)
  module Outer
    module Widget
      def self.tag = "SCOPED"
    end
  end
RUBY
module Widget
  def self.tag = "TOPLEVEL"
end
module Outer; end
Outer.autoload(:Widget, target)
module Outer
  RESULT = Widget.tag    # bare Widget here → Outer::Widget (scoped wins)
end
puts Outer::RESULT
puts Widget.tag          # toplevel constant unaffected
