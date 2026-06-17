# ERB::Util — h/html_escape + u/url_encode, as module functions and as mixed-in
# instance methods. Surfaced by rspec-core's HTML formatter (`include ERB::Util`
# for `#h`). Escaping must match CRuby's ERB::Escape (CGI.escapeHTML rules).
require "erb"

p ERB::Util.html_escape(%q{a&b<c>d"e'f})   # "a&amp;b&lt;c&gt;d&quot;e&#39;f"
p ERB::Util.h("x & y")                      # "x &amp; y"
p ERB::Util.html_escape(123)                # "123" (to_s)
p ERB::Util.url_encode("a b/c?d=e&f~g")     # "a%20b%2Fc%3Fd%3De%26f~g"
p ERB::Util.u("hello world!")               # "hello%20world%21"

# module_function makes the included #h/#html_escape PRIVATE instance methods
# (as in CRuby), so call them from within the including class.
class Helper
  include ERB::Util
  def escape(s) = h(s)
end
p Helper.new.escape("<tag>")                # "&lt;tag&gt;"
