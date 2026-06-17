# `require "English"` aliases the verbose global names to the
# punctuation match globals. rss builds method names from `$POSTMATCH`
# (`alias_method "#{$POSTMATCH}?", name`) after `require "English"`.
require "English"
/\Ais/ =~ "isPermaLink"
p $MATCH                  # "is"
p $PREMATCH               # ""
p $POSTMATCH              # "PermaLink"
"hello world" =~ /(o)(.)/
p $MATCH                  # "o "
p $PREMATCH               # "hell"
p $POSTMATCH              # "orld"
p $LAST_PAREN_MATCH       # " "  (last participating group)
# the rss idiom: derive a method name from $POSTMATCH
name = "isPermaLink"
/\Ais/ =~ name
derived = "#{$POSTMATCH}?"
p derived                 # "PermaLink?"
