# Time#yday, day-of-week predicates, #to_a, #subsec.
t = Time.at(1_700_000_000)
p t.yday
p [t.sunday?, t.monday?, t.tuesday?, t.wednesday?, t.thursday?, t.friday?, t.saturday?]
p t.to_a
p t.subsec
p Time.at(5).yday
p Time.at(0).to_a
p Time.at(1_700_000_000.5).subsec
