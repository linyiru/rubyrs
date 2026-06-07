# String#delete_prefix / delete_suffix (+ bang variants).
p "/foo/bar".delete_prefix("/")
p "/foo/bar".delete_prefix("x")
p "file.md".delete_suffix(".md")
p "file.md".delete_suffix(".x")
s = "/abc"; p s.delete_prefix!("/"); p s
t = "abc"; p t.delete_prefix!("/"); p t        # absent → nil
u = "name.scss"; p u.delete_suffix!(".scss"); p u
