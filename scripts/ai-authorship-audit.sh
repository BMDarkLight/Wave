#!/usr/bin/env bash
# Wave — AI authorship audit (reproducible)
#
# Produces a deterministic AI-authorship score from git history.
#
# Method, in short:
#   HARD   evidence = lines whose originating commit carries a machine
#                     Co-Authored-By trailer (Claude / Cursor / Copilot).
#                     Tool-generated; not inferred.
#   PROSE  evidence = comment lines bearing LLM prose markers. Counted as
#                     AI-authored *documentation*, never as AI-authored code.
#   DECLARED        = components the maintainer states are AI-authored, read
#                     from AI-AUTHORSHIP.md. Not detectable; taken on record.
#
# Explicitly NOT used: commit-level guilt-by-association (flagging a whole
# diff because one comment in it looks generated). That over-attributes by
# ~27 points on this repo and is not reproducible across auditors.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

EXCLUDE_RE='^(src-tauri/vendor/|src-tauri/gen/|docs/screenshots/|assets/|package-lock\.json|src-tauri/Cargo\.lock)'
SRC_RE='\.(rs|ts|tsx|js|jsx|css|kt|java|md|yml|yaml|toml|sh|ps1|html)$'
AI_TRAILER_RE='co-authored-by:[[:space:]]*(claude|cursor|copilot|github copilot)'
PROSE_RE='—'
COMMENT_RE='^[[:space:]]*(//|/\*|\*|#|<!--)'

TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT

git log --all --no-merges --format='%H%x01%b' \
  | awk -v RS='\n' -F'\x01' 'tolower($2) ~ /'"$AI_TRAILER_RE"'/ {print $1}' > "$TMP/ai.txt" || true
# %b is multi-line; rescan properly
: > "$TMP/ai.txt"
for c in $(git rev-list --all --no-merges); do
  if git log -1 --format='%b' "$c" | grep -qiE "$AI_TRAILER_RE"; then echo "$c" >> "$TMP/ai.txt"; fi
done
sort -o "$TMP/ai.txt" "$TMP/ai.txt"

git ls-files | grep -vE "$EXCLUDE_RE" | grep -E "$SRC_RE" | sort > "$TMP/files.txt"

: > "$TMP/lines.tsv"
while IFS= read -r f; do
  git blame --line-porcelain -w -M -C -- "$f" 2>/dev/null | awk -v F="$f" -v CR="$COMMENT_RE" -v PR="$PROSE_RE" '
    /^[0-9a-f]{40} /{sha=$1}
    /^\t/{ l=substr($0,2)
           if (l ~ /^[[:space:]]*$/) k="BLANK"
           else if (l ~ CR) k=(index(l,PR)>0 ? "CMT_AI" : "CMT")
           else k="CODE"
           print sha"\t"k"\t"F }'
done < "$TMP/files.txt" >> "$TMP/lines.tsv"

awk -F'\t' -v AIF="$TMP/ai.txt" '
BEGIN{ while((getline s < AIF)>0) ai[s]=1 }
{
  sha=$1; kind=$2; f=$3
  if (kind=="BLANK") next
  if      (f ~ /^src\//)                      comp="frontend"
  else if (f ~ /^(src-tauri\/src\/|src-tauri\/android-src\/)/) comp="backend"
  else if (f ~ /\.md$/)                       comp="docs"
  else                                        comp="build"
  tot[comp]++
  if (ai[sha]) { hard[comp]++; if (kind=="CODE") hardcode[comp]++ }
  else if (kind=="CMT_AI") prose[comp]++
}
END{
  printf "%-10s %8s %10s %10s %10s\n","component","lines","hard-AI","  of which","AI-prose"
  printf "%-10s %8s %10s %10s %10s\n","",       "",     "(trailer)"," code","(comments)"
  printf "%s\n","-------------------------------------------------------------"
  n=split("frontend backend docs build",order," ")
  for(i=1;i<=n;i++){ c=order[i]; if(!(c in tot)) continue
    printf "%-10s %8d %10d %10d %10d\n", c, tot[c], hard[c]+0, hardcode[c]+0, prose[c]+0
    T+=tot[c]; H+=hard[c]+0; P+=prose[c]+0 }
  printf "%s\n","-------------------------------------------------------------"
  printf "%-10s %8d %10d %10s %10d\n","TOTAL",T,H,"",P
  printf "\nEvidence-derived AI floor: %d / %d = %.1f%%\n", H+P, T, (H+P)*100/T
  printf "  (hard trailer evidence %.1f%% + AI-authored prose %.1f%%)\n", H*100/T, P*100/T
}' "$TMP/lines.tsv" | tee "$TMP/report.txt"

if [ -f AI-AUTHORSHIP.md ]; then
  echo
  echo "Maintainer declaration (AI-AUTHORSHIP.md):"
  sed -n '/^| /p' AI-AUTHORSHIP.md | sed 's/^/  /'
fi
