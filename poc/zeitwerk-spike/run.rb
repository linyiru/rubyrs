# Generic zeitwerk self-test runner for rubyrs.
#
# rubyrs has no `-I` flag, so load-path entries come in via the
# colon-separated `ZW_LOADPATH` env var and are prepended to
# $LOAD_PATH here. ARGV[0] is the test file to `load` (each zeitwerk
# test file `require "test_helper"` itself, which pulls in
# minitest/autorun; the at_exit hook runs the registered tests).
(ENV["ZW_LOADPATH"] || "").split(":").reverse_each { |d| $LOAD_PATH.unshift(d) unless d.empty? }
load ARGV[0]
