# Strip a leading YAML front-matter block (`---\n … \n---\n`), the way a
# static-site generator does before handing the body to the Markdown
# converter. Reads stdin, writes the body to stdout. Files without front
# matter pass through unchanged.
s = STDIN.read
if s.start_with?("---\n") && (m = s.match(/\A---\n.*?\n---\n/m))
  s = s[m.end(0)..]
end
print s
