p Encoding::GB18030.name
p Encoding.find("GB18030").name
p [Encoding::UTF_8, Encoding::US_ASCII, Encoding::GB18030].map(&:name)
