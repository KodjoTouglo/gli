#!/bin/sh
# gli uninstaller: removes the gli binary that install.sh placed on PATH.
#
#   curl -fsSL https://raw.githubusercontent.com/KodjoTouglo/gli/main/uninstall.sh | sh
#
# This removes the tool only. To undo what gli configured on a host (services,
# packages, firewall, ...), run `gli uninstall` (add --purge to also delete
# data) BEFORE removing the binary. Override the search dir with GLI_BIN_DIR.
set -eu

BIN="gli"
removed=0

for dir in "${GLI_BIN_DIR:-}" /usr/local/bin "$HOME/.local/bin"; do
  [ -n "$dir" ] || continue
  target="$dir/$BIN"
  [ -e "$target" ] || continue
  if rm -f "$target" 2>/dev/null; then
    echo "Removed $target"
    removed=1
  else
    echo "Cannot remove $target (try: sudo rm -f \"$target\")" >&2
  fi
done

if [ "$removed" -eq 0 ]; then
  echo "gli was not found in the usual locations." >&2
  echo "If you installed it elsewhere, set GLI_BIN_DIR or remove it by hand." >&2
fi
