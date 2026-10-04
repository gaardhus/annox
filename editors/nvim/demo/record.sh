#!/bin/sh
# Records demo.lua in a fresh workspace and renders it as a GIF.
# Usage: ANNOX_BIN=path/to/annox record.sh OUT.gif
# Needs asciinema 3 and agg (set AGG to use another agg binary).
set -eu
: "${ANNOX_BIN:?set ANNOX_BIN}"
out=$(realpath "$1")
here=$(cd "$(dirname "$0")" && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# A short essay with a comment and a suggestion from a collaborator.
mkdir "$tmp/ws"
cd "$tmp/ws"
cat > essay.md <<'MD'
# On reading slowly

Most of what we read today is skimmed, not read. We scan headlines,
pick out a few keywords, and move on before the argument has a chance
to land.

Reading slowly is a kind of resistance. It asks for patience, and it
gives back something that speed never could: the feeling of having
actually understood a thing.

A good essay rewards a second pass. The first time through you follow
the argument; the second time you notice how it was built.
MD
"$ANNOX_BIN" init . >/dev/null
export ANNOX_AUTHOR=mailto:ada@example.org ANNOX_AUTHOR_NAME=Ada
"$ANNOX_BIN" comment essay.md --quote "a kind of resistance" \
  --body "Resistance to what, exactly? Worth one more sentence." >/dev/null
"$ANNOX_BIN" suggest essay.md --quote "something that speed never could" \
  --replace "something speed cannot" --body "Tighter." >/dev/null

# asciicast v2, which agg reads before 1.5 too. The recording terminal doesn't
# answer Neovim's startup queries, so skip them (else E1568 shows).
NVIM_NOTTYFAST=1 asciinema rec --headless -q -f asciicast-v2 --window-size 112x16 \
  -c "nvim -u '$here/demo.lua' essay.md" "$tmp/demo.cast"
"${AGG:-agg}" --font-family "BlexMono Nerd Font Mono,JetBrains Mono,DejaVu Sans Mono" \
  --font-size 20 --last-frame-duration 1 "$tmp/demo.cast" "$out" >/dev/null 2>&1
echo "wrote $out"
