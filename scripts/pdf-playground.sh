#!/usr/bin/env bash
# Builds a playground for trying PDF sync by hand in DIR (default /tmp/annox-pdf-playground),
# replacing an earlier one there. docs/testing/pdf-sync.md says what to try in it.
#
#   hub/           the hub's data, for `annox hub`
#   sam/ jane/     two copies of examples/proposal.pdf, each a workspace that syncs through the hub
#   okular-check/  a copy on its own, no hub, with a few threads already written into its PDF
#
# Each copy has an env.sh to source in its terminals: it sets who you are there, and puts the
# annox at $ANNOX (default: the one on the PATH) first on the PATH.
set -euo pipefail

dir=${1:-/tmp/annox-pdf-playground}
repo=$(cd "$(dirname "$0")/.." && pwd)
annox=$(command -v "${ANNOX:-annox}") || { echo "no annox at ${ANNOX:-annox}" >&2; exit 1; }
url=ws://127.0.0.1:7878/w/proposal

if [ -e "$dir" ]; then
    [ -e "$dir/.annox-playground" ] || { echo "$dir exists and isn't a playground; not touching it" >&2; exit 1; }
    rm -rf "$dir"
fi
mkdir -p "$dir/hub"
touch "$dir/.annox-playground"
dir=$(cd "$dir" && pwd)
printf '{ "hubs": { "%s": {} } }\n' "$url" > "$dir/credentials.json"

# A copy of the example PDF in $1, as a workspace, with an env.sh for author $2 named $3.
copy() {
    mkdir -p "$1"
    cp "$repo/examples/proposal.pdf" "$1/"
    git -C "$1" init -q
    "$annox" init "$1" > /dev/null
    cat > "$1/env.sh" <<EOF
# Source this in each terminal for this copy: \`source env.sh\`
export PATH="$(dirname "$annox"):\$PATH"
export ANNOX_CREDENTIALS_FILE="$dir/credentials.json"
export ANNOX_AUTHOR="$2" ANNOX_AUTHOR_NAME="$3"
threads() { annox list proposal.pdf "\$@"; }
EOF
}

for who in sam:Sam jane:Jane; do
    copy "$dir/${who%%:*}" "mailto:${who%%:*}@example.com" "${who#*:}"
    printf '{ "format": 1, "sync": { "url": "%s" } }\n' "$url" > "$dir/${who%%:*}/.annox/annox.json"
done

copy "$dir/okular-check" mailto:you@example.com You
(
    cd "$dir/okular-check"
    as() { ANNOX_AUTHOR="mailto:$1@example.com" ANNOX_AUTHOR_NAME="${1^}" "${@:2}"; }
    week=$(as sam "$annox" comment proposal.pdf --quote "about a week" --body "Too short?" --json | jq -r .id)
    as jane "$annox" reply "$week" --body "Agreed, say two weeks." > /dev/null
    as jane "$annox" comment proposal.pdf --quote "Search will be worse at first" \
        --body $'Two things here.\n\nFirst, how much worse? Second, who decides when it is good enough?' > /dev/null
    as sam "$annox" suggest proposal.pdf --quote "one month" --replace "two months" \
        --body "People need time to notice." > /dev/null
    as jane "$annox" highlight proposal.pdf --quote "every change is reviewed" > /dev/null
    seats=$(as sam "$annox" comment proposal.pdf --quote "40 seats" --body "Is this still right?" --json | jq -r .id)
    as jane "$annox" reply "$seats" --body "Yes, checked the invoice." > /dev/null
    as sam "$annox" status "$seats" resolved > /dev/null
    as sam "$annox" sync-pdf proposal.pdf > /dev/null
)

echo "Playground in $dir, using $annox. See docs/testing/pdf-sync.md."
