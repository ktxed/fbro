#!/usr/bin/env bash

set -euo pipefail

if [[ ${1:-} == "-h" || ${1:-} == "--help" ]]; then
  cat <<'EOF'
Usage: tools/generate-live-files.sh [DIRECTORY]

Create one new file every second until interrupted with Ctrl-C.
DIRECTORY defaults to /tmp/fbro-live-files. Existing files are left untouched.
EOF
  exit 0
fi

target_dir=${1:-/tmp/fbro-live-files}
mkdir -p -- "$target_dir"
target_dir=$(cd -- "$target_dir" && pwd -P)

printf 'Generating one file per second in: %s\n' "$target_dir"
printf 'Open this directory in fbro and sort by Date descending.\n'
printf 'Press Ctrl-C to stop. Existing and generated files will not be deleted.\n'
printf 'Cleanup when finished: rm -rf -- %q\n' "$target_dir"

sequence=0
trap 'printf "\nStopped after %d files.\n" "$sequence"; exit 0' INT TERM

while true; do
  sequence=$((sequence + 1))
  timestamp=$(date +%Y%m%d-%H%M%S)
  filename=$(printf 'generated_%s_%06d.txt' "$timestamp" "$sequence")
  printf 'Generated at %s\nSequence: %d\n' "$(date '+%Y-%m-%dT%H:%M:%S%z')" "$sequence" \
    > "$target_dir/$filename"
  printf '%s\n' "$filename"
  sleep 1
done
