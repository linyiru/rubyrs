# `require 'uri'` stub. base.rb's `referrer` helper parses
# HTTP_REFERER via URI; none of the middlewares we ship
# (FrameOptions / XSSHeader / PathTraversal) consult it. Stub
# both the URI namespace and the InvalidURIError class the
# rescue clause references.
module URI
  class InvalidURIError < StandardError; end
  def self.parse(_)
    # Returns nil so callers reading `.host` get nil — they're
    # guarded with `rescue URI::InvalidURIError` in the original
    # source so a raise here would just route through the same
    # fallback.
    nil
  end
end
