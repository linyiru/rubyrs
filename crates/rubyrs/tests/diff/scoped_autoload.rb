# A scoped `autoload :Inner, file` is fired by a COLD reference to a
# constant NESTED under it (`Outer::Inner::DEEP`), via the `::`-prefix
# walk — not only by a bare `Outer::Inner`. Mirrors jekyll's
# `Document::DATE_FILENAME_MATCHER` reaching the `autoload :Document`.
module ScopedOuter
  autoload :Inner, File.join(__dir__, "scoped_autoload_helper")
end
p ScopedOuter::Inner::DEEP        # cold nested ref fires the autoload
p ScopedOuter::Inner.greet
