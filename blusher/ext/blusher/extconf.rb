# frozen_string_literal: true

# rb-sys driven build: rake-compiler invokes this at `gem install`, and
# `create_rust_makefile` emits a Makefile that runs `cargo build` and installs
# the cdylib as the gem's loadable object. The argument names the output so it
# lands at `lib/blusher/blusher.<dlext>` and is required as "blusher/blusher".
require "mkmf"
require "rb_sys/mkmf"

create_rust_makefile("blusher/blusher")
