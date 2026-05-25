# Required by require_relative_main.rb's outer-rescue test.
# Raises mid-load so the caller's begin/rescue catches it —
# exercises the unwind-past-require_relative path that would
# otherwise corrupt the rescue handler's operand stack.
raise "boom from required file"
