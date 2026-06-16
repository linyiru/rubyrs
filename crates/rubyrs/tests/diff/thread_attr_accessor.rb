# `Thread.attr_accessor :x` + `Thread.current.x` round-trips. In CRuby
# `Thread.current` is a thread instance; in rubyrs's single-thread model
# it's the Thread class, but the class-level accessor makes the
# observable behaviour converge. Surfaced by bridgetown-core/current.rb
# (`Thread.attr_accessor :bridgetown_state; Thread.current.bridgetown_state ||= {}`).
Thread.attr_accessor :app_state
Thread.current.app_state ||= {}
Thread.current.app_state[:sites] ||= {}
Thread.current.app_state[:sites][:main] = "site"
p Thread.current.app_state
puts Thread.current.app_state[:sites][:main]
