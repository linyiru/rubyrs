# `require "io/console/size"` loads, and IO.default_console_size
# honors $LINES / $COLUMNS with the 25x80 fallback. (IO.console_size
# itself may read a real tty's winsize on CRuby, so only its shape
# is checked.)
require "io/console/size"

ENV.delete("LINES")
ENV.delete("COLUMNS")
p IO.default_console_size
ENV["LINES"] = "40"
ENV["COLUMNS"] = "120"
p IO.default_console_size
ENV["COLUMNS"] = "0"
p IO.default_console_size
size = IO.console_size
p [size.size, size.all?(Integer)]
