# Generic zeitwerk self-test runner for rubyrs.
#
# rubyrs has no `-I` flag, so load-path entries come in via the
# colon-separated `ZW_LOADPATH` env var and are prepended to
# $LOAD_PATH here. ARGV[0] is the test file to `load` (each zeitwerk
# test file `require "test_helper"` itself, which pulls in
# minitest/autorun; the at_exit hook runs the registered tests).
(ENV["ZW_LOADPATH"] || "").split(":").reverse_each { |d| $LOAD_PATH.unshift(d) unless d.empty? }

# Faithfulness to zeitwerk's native suite (where these hold in-process):
#  - `pathname` is used by a few tests (file_system/test_helpers) without an
#    explicit require; the full suite loads it via another file, our single-file
#    harness loads it here.
#  - load through the realpath'd path so `__FILE__` matches `__dir__` (which
#    realpaths). On macOS `/tmp` symlinks to `/private/tmp`, so otherwise
#    test_ignore's `__FILE__`-vs-`__dir__` comparisons diverge (CRuby fails it
#    identically without this; zeitwerk's Linux CI has no such symlink).
require "pathname"
load File.realpath(ARGV[0])
