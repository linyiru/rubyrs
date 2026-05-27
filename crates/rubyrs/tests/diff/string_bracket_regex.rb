# `String#[regex]` / `String#[regex, n]` — Regex overloads of `[]`.
#
# Motivating consumer: tilt's `extract_magic_comment` at
# tilt-2.7.0 lib/tilt/template.rb:1937 does
#
#   script[/\A[ \t]*\#.*coding\s*[=:]\s*([[:alnum:]\-_]+).*$/n, 1]
#
# to pull the encoding name out of an ERB-generated `#coding:UTF-8`
# header. Without the (Regex, Int) form, the full tilt render path
# fails before it can compile the template method.
#
# Coverage:
#   - 1-arg: whole-match return, nil on no-match
#   - 2-arg with capture index: 0 = whole, n>0 = n-th group,
#     out-of-range = nil, no-match = nil
#   - tilt-shape probe (magic-comment extraction, the real call)
#   - $~ / $& / $1 side channel still updated, mirroring #match

# --- 1-arg Regex: whole match or nil ---
puts "hello world"[/w\w+/]                      # world
puts "hello world"[/xyz/].inspect               # nil

# --- 2-arg (Regex, 0): whole match ---
puts "hello world"[/w(\w+)/, 0]                 # world

# --- 2-arg (Regex, 1): first capture ---
puts "hello world"[/w(\w+)/, 1]                 # orld

# --- 2-arg (Regex, n): n-th capture ---
puts "abc123def"[/([a-z]+)(\d+)([a-z]+)/, 2]    # 123
puts "abc123def"[/([a-z]+)(\d+)([a-z]+)/, 3]    # def

# --- Out-of-range capture index → nil ---
puts "hello"[/(h)/, 5].inspect                  # nil

# --- No match, 2-arg → nil ---
puts "hello"[/xyz/, 1].inspect                  # nil

# --- tilt extract_magic_comment shape (without trailing `.*$`) ---
# tilt's real regex is
#   /\A[ \t]*\#.*coding\s*[=:]\s*([[:alnum:]\-_]+).*$/n
# but the trailing `.*$` triggers a pre-existing rubyrs regex
# divergence (Rust `regex` defaults to `$` = end of input, CRuby's
# default is end of line). The capture itself — POSIX char class
# `[[:alnum:]]` + escaped `\-` + literal `_` — is what we care
# about exercising here.
src = "#coding:UTF-8\n_erbout = +''"
puts src[/\A[ \t]*\#.*coding\s*[=:]\s*([[:alnum:]\-_]+)/n, 1]      # UTF-8

# --- Side-channel: $~ / $& / $1 updated as if #match had been called ---
"alpha 42 beta"[/(\w+)\s(\d+)/]
puts $&                                         # alpha 42
puts $1                                         # alpha
puts $2                                         # 42

# --- "slice" alias supports the same overloads ---
puts "hello world".slice(/w\w+/)                # world
puts "abc123".slice(/([a-z]+)(\d+)/, 2)         # 123
