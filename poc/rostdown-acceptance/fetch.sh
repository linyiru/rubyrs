#!/usr/bin/env bash
# Fetch permissively-licensed real-world Markdown into a gitignored local
# corpus and prepare it for the acceptance scan. Nothing is vendored — the
# sources are cloned on demand and `prepared/` holds only front-matter-
# stripped bodies for measurement, both ignored by git. See README.md for
# sources, licenses, and caveats.
set -euo pipefail
cd "$(dirname "$0")"

CORPUS=corpus
PREP=prepared
mkdir -p "$CORPUS" "$PREP"

# name | git url | space-separated content roots | SPDX license
SOURCES=(
  "jekyll|https://github.com/jekyll/jekyll.git|docs/_posts docs/_docs docs/_tutorials|MIT"
  "bridgetown|https://github.com/bridgetownrb/bridgetown.git|bridgetown-website/src/_posts bridgetown-website/src/_docs|MIT"
)

for entry in "${SOURCES[@]}"; do
  IFS='|' read -r name url roots license <<<"$entry"
  if [ ! -d "$CORPUS/$name/.git" ]; then
    echo "cloning $name ($license) …"
    rm -rf "${CORPUS:?}/$name"
    git clone --depth 1 -q "$url" "$CORPUS/$name"
  else
    echo "$name already cloned (rm -rf $CORPUS/$name to refresh)"
  fi
  rm -rf "${PREP:?}/$name"
  mkdir -p "$PREP/$name"
  count=0
  for root in $roots; do
    [ -d "$CORPUS/$name/$root" ] || continue
    while IFS= read -r f; do
      base=$(printf '%s' "${f#"$CORPUS/$name/"}" | tr '/' '_')
      ruby strip_frontmatter.rb <"$f" >"$PREP/$name/$base"
      count=$((count + 1))
    done < <(find "$CORPUS/$name/$root" -type f \( -name '*.md' -o -name '*.markdown' \))
  done
  echo "  $name: $count files prepared ($license)"
done
echo
echo "done. now run ./run.sh"
