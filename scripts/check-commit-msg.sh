#!/bin/sh
# Conventional-commit gate: <type>(<scope>)?: <lowercase imperative summary>
# of at most 72 characters. Merge and release subjects pass through.
pattern='^(feat|fix|perf|refactor|test|docs|chore|ci|build|revert)(\([a-z0-9-]+\))?: [a-z].{0,70}$'
subject=$(head -n1 "$1")
case "$subject" in
  "Merge "*|"Release "*v*) exit 0 ;;
esac
if ! printf '%s' "$subject" | grep -Eq "$pattern"; then
  echo "commit message rejected: $subject" >&2
  echo "expected: type(scope)?: lowercase imperative summary, max 72 chars" >&2
  echo "types: feat fix perf refactor test docs chore ci build revert" >&2
  exit 1
fi
