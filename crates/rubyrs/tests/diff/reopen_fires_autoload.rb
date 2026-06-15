# Reopening a module/class that has a PENDING AUTOLOAD must fire the
# autoload first (CRuby semantics): the existing definition — and any
# constants the autoloaded file adds — must be present, then the reopen
# augments it. Surfaced by bridgetown-foundation, where opening
# `module …RefineExt` (a zeitwerk-autoloaded namespace) has to trigger
# the registration of its sibling refine_ext files.
target = "/tmp/rubyrs_reopen_autoload_target.rb"
File.write(target, <<~RUBY)
  module Foo
    FROM_AUTOLOAD = :loaded
    def self.helper = :helper
  end
RUBY
autoload :Foo, target
module Foo
  ADDED_IN_REOPEN = :reopened
end
p Foo.constants.sort
p Foo::FROM_AUTOLOAD
p Foo::ADDED_IN_REOPEN
p Foo.helper
