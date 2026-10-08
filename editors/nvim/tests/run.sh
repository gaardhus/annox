#!/usr/bin/env bash
# Runs every plugin test, or the ones given, against $ANNOX_BIN.
# Run: ANNOX_BIN=$PWD/target/debug/annox editors/nvim/tests/run.sh [test.lua...]
#
# A test passes only if it prints its "annox nvim <name>: OK" line. Headless
# Neovim can stop a script early with exit code 0, for example at a prompt the
# test didn't answer, so the exit code alone isn't enough.
set -uo pipefail

: "${ANNOX_BIN:?set ANNOX_BIN}"
ANNOX_BIN=$(realpath "$ANNOX_BIN") # tests change directory
export ANNOX_BIN

here=$(dirname "$0")
if [ $# -eq 0 ]; then
  set -- "$here"/*.lua
fi

failed=0
for t in "$@"; do
  name=$(basename "$t" .lua)
  [ -n "${GITHUB_ACTIONS:-}" ] && echo "::group::$t"
  out=$(nvim --headless --clean -l "$t" < /dev/null 2>&1)
  status=$?
  [ -n "${GITHUB_ACTIONS:-}" ] && printf '%s\n' "$out" && echo "::endgroup::"
  if [ $status -ne 0 ] || ! grep -aq "annox nvim $name: OK" <<< "$out"; then
    echo "FAIL $t (exit $status, no OK line). The end of its output:"
    printf '%s\n' "$out" | tail -n 20
    failed=1
  else
    echo "ok   $t"
  fi
done
exit $failed
