# Focused CGI module: the escape/unescape surface Liquid's standard
# filters (escape → CGI.escapeHTML, url_encode → CGI.escape) and the
# Jekyll template chain actually call. Not the full CGI class — no
# request/response machinery (out of subset).
module CGI
  HTML_ESCAPE = {
    "&" => "&amp;", "<" => "&lt;", ">" => "&gt;",
    '"' => "&quot;", "'" => "&#39;",
  }.freeze

  def self.escapeHTML(str)
    out = +""
    str.to_s.each_char do |c|
      out << (HTML_ESCAPE[c] || c)
    end
    out
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
