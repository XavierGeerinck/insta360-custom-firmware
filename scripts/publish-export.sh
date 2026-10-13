#!/usr/bin/env bash
# Builds the public copy of one branch: its full history with everything that
# must not be published taken out. Pushes nothing; it ends by printing the push.
#
#   scripts/publish-export.sh [ref] [out-dir]
#
# ref      branch to publish (default: the current branch); it becomes `main`
# out-dir  where the export goes (default: a new temporary directory)
#
# What it removes, in every commit:
#   - decompiled/, firmware/ and waved/rust_out (Insta360's code and files)
#   - every value in the scrub rules (WAVE_SCRUB_RULES, default
#     ~/.config/waved/public-scrub.txt; git-filter-repo --replace-text format,
#     kept outside the repository), in file contents and commit messages
#   - the `decompiled_c` fields of docs/npu/**/*ghidra-evidence.json (Insta360's
#     code, decompiled by Ghidra); the rest of each file stays
# Then it checks the result and stops on any leftover.
#
# The rewrite is deterministic: the same input and rules give the same commits,
# so publishing a later state of the same branch is a fast-forward push.
set -euo pipefail

REPO="$(git rev-parse --show-toplevel)"
REF="${1:-$(git -C "$REPO" rev-parse --abbrev-ref HEAD)}"
OUT="${2:-$(mktemp -d)/wave-public}"
RULES="${WAVE_SCRUB_RULES:-$HOME/.config/waved/public-scrub.txt}"

[ -f "$RULES" ] && [ -r "$RULES" ] || { echo "no scrub rules at $RULES" >&2; exit 2; }
command -v git-filter-repo >/dev/null 2>&1 || { echo "git-filter-repo is not installed" >&2; exit 2; }
[ ! -e "$OUT" ] || { echo "$OUT already exists" >&2; exit 2; }

echo "=== Cloning $REF ==="
git clone --quiet --no-local --single-branch --branch "$REF" "$REPO" "$OUT"
cd "$OUT"
[ "$REF" = main ] || git branch -q -m "$REF" main
git remote remove origin

echo "=== Removing vendor paths and scrubbed values ==="
git filter-repo --quiet --force \
	--invert-paths --path decompiled/ --path firmware/ --path waved/rust_out \
	--replace-text "$RULES" --replace-message "$RULES"

echo "=== Stripping decompiled code from the NPU evidence files ==="
git filter-repo --quiet --force --file-info-callback '
if filename.startswith(b"docs/npu/") and filename.endswith(b"ghidra-evidence.json"):
    import json
    raw = value.get_contents_by_identifier(blob_id).decode("utf-8")
    data = json.loads(raw)
    def strip(o):
        if isinstance(o, dict):
            o.pop("decompiled_c", None)
            for v in o.values():
                strip(v)
        elif isinstance(o, list):
            for v in o:
                strip(v)
    strip(data)
    text = json.dumps(data, indent=2, ensure_ascii=raw.isascii()) + "\n"
    blob_id = value.insert_file_with_contents(text.encode("utf-8"))
return (filename, mode, blob_id)
'

echo "=== Checking the export ==="
python3 -I - "$RULES" <<'PY'
import re
import subprocess
import sys

rules = [l.rstrip("\n").split("==>")[0] for l in open(sys.argv[1]) if l.strip()]
history = subprocess.run(["git", "log", "--all", "-p", "--format=%H%n%B"],
                         capture_output=True, text=True, errors="ignore").stdout
problems = []
for n, rule in enumerate(rules, 1):
    if rule.startswith("regex:"):
        hits = len(re.findall(rule[6:], history))
    else:
        hits = history.count(rule) + history.lower().count(rule.lower()) * (rule.lower() != rule)
    if hits:
        problems.append(f"rule {n}: {hits} occurrence(s) left in history")
paths = subprocess.run(["git", "log", "--all", "--format=%H", "--", "decompiled", "firmware", "waved/rust_out"],
                       capture_output=True, text=True).stdout.split()
if paths:
    problems.append(f"{len(paths)} commit(s) still touch decompiled/, firmware/ or waved/rust_out")
if '"decompiled_c"' in subprocess.run(["git", "log", "--all", "-p", "--", "docs/npu"],
                                      capture_output=True, text=True, errors="ignore").stdout:
    problems.append("decompiled_c still appears under docs/npu")
for p in problems:
    print("LEFTOVER:", p)
print(f"checked {len(rules)} rules over {history.count(chr(10))} lines of history")
sys.exit(1 if problems else 0)
PY

if command -v gitleaks >/dev/null 2>&1; then
	gitleaks git --log-opts="--all" --no-banner --redact . >/dev/null 2>&1 && echo "gitleaks: no findings" ||
		echo "gitleaks: findings (review with: cd $OUT && gitleaks git --log-opts=--all --redact .)"
fi

echo
echo "Export ready: $OUT ($(git rev-list --count main) commits, main at $(git rev-parse --short main))"
echo "To publish:  git -C $OUT push <public repo url> main"
