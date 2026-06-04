# `require 'digest'` stub. base.rb declares `Digest::SHA1` as the
# default encryptor option, but our three middlewares
# (FrameOptions / XSSHeader / PathTraversal) don't reach the
# `encrypt` helper, so the constant just needs to BE there.
module Digest
  module SHA1
    def self.hexdigest(_); ""; end
  end
end
