# `Regexp#encoding` — US-ASCII for an all-ASCII source, UTF-8 when the
# pattern has multibyte chars. Surfaced by regexp_parser's scanner
# (`extract_encoding` reads `regexp.encoding`).
p(/abc/.encoding)              # #<Encoding:US-ASCII>
p(/a.c\d+/.encoding)           # #<Encoding:US-ASCII>
p(/abç/.encoding)              # #<Encoding:UTF-8>
p(/\A\d+\z/.encoding.name)     # "US-ASCII"
p(/héllo/.encoding.name)       # "UTF-8"
p(Regexp.new("xyz").encoding)  # #<Encoding:US-ASCII>
p(//.encoding)                 # #<Encoding:US-ASCII>  (empty)
