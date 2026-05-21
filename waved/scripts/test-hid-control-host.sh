#!/usr/bin/env bash
# SP#7(d') host-side companion to scripts/test-hid-control.sh (macOS).
set -euo pipefail

osx_get_vol()   { osascript -e 'output volume of (get volume settings)' 2>/dev/null || echo "?"; }
osx_get_muted() { osascript -e 'output muted of (get volume settings)' 2>/dev/null || echo "?"; }

echo "host volume before: $(osx_get_vol)"
echo "host muted before:  $(osx_get_muted)"
echo
echo "Now run scripts/test-hid-control.sh on the device."
echo "Press [Return] when the device script completes."
read -r _
echo
echo "host volume after:  $(osx_get_vol)"
echo "host muted after:   $(osx_get_muted)"
