#!/usr/bin/env bash
# Counts, for each value in the scrub file, the tracked files that still
# contain it. Prints nothing and exits 0 when the tree is clean. The values
# themselves are never printed.
# Usage: scripts/check-scrub.sh [scrub file]
set -uo pipefail
FILE="${1:-$HOME/.config/waved/public-scrub.txt}"
dirty=0; n=0
while IFS= read -r line || [ -n "$line" ]; do
    [ -z "$line" ] && continue
    n=$((n+1))
    from="${line%%==>*}"
    if [[ "$from" == regex:* ]]; then hits=$(git grep -l -E -- "${from#regex:}" | wc -l)
    else hits=$(git grep -l -F -- "$from" | wc -l); fi
    if [ "$hits" -gt 0 ]; then echo "rule $n: $hits file(s)"; dirty=1; fi
done < "$FILE"
[ -e decompiled ] && git ls-files --error-unmatch decompiled >/dev/null 2>&1 && { echo "decompiled/ is tracked"; dirty=1; }
exit $dirty
