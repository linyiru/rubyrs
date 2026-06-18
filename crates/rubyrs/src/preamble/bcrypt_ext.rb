# `_bcrypt` battery — stands in for the bcrypt gem's `bcrypt_ext` C
# extension. Defines `BCrypt::Engine` with the two private class methods
# the gem's pure-Ruby `engine.rb` calls (`__bc_salt` / `__bc_crypt`),
# delegating to the native pure-Rust EksBlowfish implementation
# (src/bcrypt.rs) via the registered host fns. engine.rb later reopens
# this class to add the Ruby surface and mark these two private.
module BCrypt
  class Engine
    def self.__bc_salt(prefix, cost, input)
      __rubyrs_bcrypt_salt(prefix, cost, input)
    end

    def self.__bc_crypt(secret, salt)
      __rubyrs_bcrypt_crypt(secret, salt)
    end
  end
end
