if /(?<year>\d+)-(?<mon>\d+)/ =~ "2020-05"
  p [year, mon]
end
r = (/(?<a>\w)(?<b>\w)/ =~ "xy")
p r
p [a, b]
res = (/(?<x>\d+)/ =~ "abc")
p res
p x
# only named captures bind; unnamed groups don't create locals
/(?<n>\d+)(\w+)/ =~ "12ab"
p n
