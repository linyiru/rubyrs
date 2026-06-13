# Focused CGI module: the escape/unescape surface Liquid's standard
# filters (escape → CGI.escapeHTML, url_encode → CGI.escape) and the
# Jekyll template chain actually call. Not the full CGI class — no
# request/response machinery (out of subset).
module CGI
  HTML_ESCAPE = {
    "&" => "&amp;", "<" => "&lt;", ">" => "&gt;",
    '"' => "&quot;", "'" => "&#39;",
  }.freeze

  # BYTE-level on purpose: the five escapees are ASCII, and a char
  # walk pushes invalid-UTF-8 input through the lossy char view
  # (U+FFFD), where CRuby's escapeHTML preserves the raw bytes —
  # rack's spec pins `escape_html("\xC0<")` == "\xC0&lt;" with the
  # invalid byte intact. Same pattern as the uri shim's www-form
  # pair; the result keeps the receiver's encoding tag (including
  # its invalid-ness), matching CRuby.
  ESCAPE_HTML_BYTES = {
    38 => "&amp;".bytes.freeze, 60 => "&lt;".bytes.freeze,
    62 => "&gt;".bytes.freeze, 34 => "&quot;".bytes.freeze,
    39 => "&#39;".bytes.freeze,
  }.freeze

  def self.escapeHTML(str)
    str = str.to_s
    bytes = []
    str.each_byte do |b|
      if esc = ESCAPE_HTML_BYTES[b]
        bytes.concat(esc)
      else
        bytes << b
      end
    end
    bytes.pack("C*").force_encoding(str.encoding)
  end

  def self.unescapeHTML(str)
    str.to_s
       .gsub("&amp;", "\0AMP\0")
       .gsub("&lt;", "<").gsub("&gt;", ">")
       .gsub("&quot;", '"').gsub("&#39;", "'").gsub("&#x27;", "'")
       .gsub("\0AMP\0", "&")
  end

  # RFC 3986-ish form escaping (CRuby CGI.escape: unreserved chars
  # pass, space becomes "+", everything else %XX uppercase).
  def self.escape(str)
    out = +""
    str.to_s.each_byte do |b|
      c = b.chr
      if c =~ /[A-Za-z0-9\-_.~*]/
        out << c
      elsif c == " "
        out << "+"
      else
        out << sprintf("%%%02X", b)
      end
    end
    out
  end

  def self.unescape(str)
    str.to_s.gsub("+", " ").gsub(/%([0-9A-Fa-f]{2})/) { $1.to_i(16).chr }
  end
end
