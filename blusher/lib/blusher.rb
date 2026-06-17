# frozen_string_literal: true

require_relative "blusher/version"
require_relative "blusher/native"
require_relative "blusher/shim" # installs the rouge-API drop-in on require

# blusher — a fast, rouge-compatible syntax highlighter. Requiring it routes
# rouge's lexers through the Rust `carmine` engine where it produces a
# byte-identical token stream, and falls back to rouge itself everywhere else
# (zero code change, zero divergence). See README.
module Blusher
  # Lex `source` with the lexer for `tag` (e.g. "ruby"), returning rouge
  # [Token, value] pairs — via carmine when supported, else rouge.
  def self.lex(tag, source)
    lexer = Rouge::Lexer.find(tag) or raise ArgumentError, "unknown lexer: #{tag}"
    lexer.new.lex(source).to_a
  end
end
