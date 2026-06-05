#!/usr/bin/env sh
set -eu

drop_bytes=1048576000

usage() {
  printf 'Usage: %s FILE\n' "$0" >&2
}

if [ "$#" -ne 1 ]; then
  usage
  exit 2
fi

file=$1

if [ ! -e "$file" ]; then
  printf 'error: not found: %s\n' "$file" >&2
  exit 1
fi

if [ ! -f "$file" ]; then
  printf 'error: not a regular file: %s\n' "$file" >&2
  exit 1
fi

size=$(stat -f %z "$file" 2>/dev/null || stat -c %s "$file")

if [ "$size" -lt "$drop_bytes" ]; then
  printf 'error: file is smaller than 1000 MiB (%s bytes): %s\n' "$size" "$file" >&2
  exit 1
fi

new_size=$((size - drop_bytes))
truncate -s "$new_size" "$file"

printf 'truncated %s from %s to %s bytes\n' "$file" "$size" "$new_size"
