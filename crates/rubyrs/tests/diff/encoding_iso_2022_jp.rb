# The ISO-2022-JP constant exists (a legacy CRuby "dummy" encoding). Rack
# 3.2.6's multipart parser builds `{ Encoding::ISO_2022_JP => true }` at
# load, so requiring rack/Sinatra referenced an undefined constant.
p defined?(Encoding::ISO_2022_JP)
p Encoding::ISO_2022_JP.name
p Encoding.find("ISO-2022-JP").name
p Encoding.find("ISO-2022-JP").equal?(Encoding::ISO_2022_JP)
p({ Encoding::ISO_2022_JP => true }[Encoding.find("ISO-2022-JP")])
