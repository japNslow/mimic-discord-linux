#!/bin/sh
set -eu
mimic_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if [ -f "$mimic_dir/mimic-linux" ] && [ -x "$mimic_dir/mimic-linux" ]; then
  exec "$mimic_dir/mimic-linux" "$@"
elif [ -x "$mimic_dir/dist-linux/mimic-linux" ]; then
  exec "$mimic_dir/dist-linux/mimic-linux" "$@"
elif [ -x "$mimic_dir/src-tauri/target/release/mimic-linux" ]; then
  exec "$mimic_dir/src-tauri/target/release/mimic-linux" "$@"
fi
printf '%s\n' 'Сначала соберите приложение: npm ci --ignore-scripts && npm run linux:portable' >&2
exit 1
