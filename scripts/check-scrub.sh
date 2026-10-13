#!/usr/bin/env bash
# Counts, for each value in the scrub file, the tracked files that still
# contain it. Prints nothing and exits 0 when the tree is clean. The values
# themselves are never printed: git's stderr is discarded (a malformed regex
# makes git echo the pattern) and failures name only the rule number.
# Exit status: 0 clean, 1 leaks found, 2 usage or rule error.
# Usage: scripts/check-scrub.sh [scrub file]
set -uo pipefail
FILE="${1:-$HOME/.config/waved/public-scrub.txt}"
[ -r "$FILE" ] || { echo "no scrub file"; exit 2; }
cd "$(git rev-parse --show-toplevel 2>/dev/null)" || { echo "not in a git work tree"; exit 2; }
dirty=0; broken=0; n=0
while IFS= read -r line || [ -n "$line" ]; do
    [ -z "$line" ] && continue
    n=$((n+1))
    from="${line%%==>*}"
    if [[ "$from" == regex:* ]]; then out=$(git grep -l -E -- "${from#regex:}" 2>/dev/null); rc=$?
    else out=$(git grep -l -F -- "$from" 2>/dev/null); rc=$?; fi
    # git grep: 0 = match, 1 = no match, >=2 = error (e.g. a malformed regex)
    if [ "$rc" -ge 2 ]; then echo "rule $n: could not be evaluated"; broken=1; continue; fi
    hits=0; [ -n "$out" ] && hits=$(printf '%s\n' "$out" | wc -l | tr -d ' ')
    if [ "$hits" -gt 0 ]; then echo "rule $n: $hits file(s)"; dirty=1; fi
done < "$FILE"
[ "$n" -gt 0 ] || { echo "no rules in scrub file"; exit 2; }
git ls-files --error-unmatch decompiled >/dev/null 2>&1 && { echo "decompiled/ is tracked"; dirty=1; }
[ "$broken" -eq 0 ] || exit 2
exit $dirty
