#!/usr/bin/env bash
# Generates llms.txt and llms-full.txt from mdBook source files,
# and copies source .md files into the build output for direct access.
set -euo pipefail

DOCS_DIR="$(cd "$(dirname "$0")" && pwd)"
SRC_DIR="$DOCS_DIR/src"
BOOK_DIR="$DOCS_DIR/book/html"
SITE_URL="https://mojzis.github.io/pycoati"

mkdir -p "$BOOK_DIR"

# Print a source page with mdBook's {{#include path}} lines expanded, so a page
# that pulls in a repo file (weights.md -> WEIGHTS.md) is served in full.
render() {
    local file="$1" dir
    dir="$(dirname "$file")"
    while IFS= read -r line || [ -n "$line" ]; do
        if [[ "$line" =~ ^\{\{#include\ ([^}]+)\}\}$ ]]; then
            cat "$dir/${BASH_REMATCH[1]}"
        else
            printf '%s\n' "$line"
        fi
    done < "$file"
}

# Copy source .md files into the build output so they're served alongside HTML.
# Preserve directory structure (e.g. commands/*.md).
grep -oP '\(([^)]+\.md)\)' "$SRC_DIR/SUMMARY.md" | tr -d '()' | while IFS= read -r path; do
    if [ -f "$SRC_DIR/$path" ]; then
        mkdir -p "$BOOK_DIR/$(dirname "$path")"
        render "$SRC_DIR/$path" > "$BOOK_DIR/$path"
    fi
done
echo "Copied .md source files to $BOOK_DIR/"

# llms.txt — index with links to .md files
cat > "$BOOK_DIR/llms.txt" << EOF
# pycoati

> Audits Python test suites for mock smells and suspicious tests: scores every
> test for suspicion and hands a coding agent a ranked list plus a remediation
> ladder. Also plans parallel pytest runs for monorepos.

## Documentation

EOF

# Parse SUMMARY.md to extract chapter links and titles.
# Matches both standalone links like "[Title](path.md)" and list items like "- [Title](path.md)".
grep -oP '\[([^\]]+)\]\(([^)]+)\)' "$SRC_DIR/SUMMARY.md" | while IFS= read -r match; do
    title=$(echo "$match" | sed 's/\[\([^]]*\)\](.*)/\1/')
    path=$(echo "$match" | sed 's/\[.*\](\(.*\))/\1/')
    echo "- [$title]($SITE_URL/$path): $title" >> "$BOOK_DIR/llms.txt"
done

echo "Wrote $BOOK_DIR/llms.txt"

# llms-full.txt — all source files concatenated
: > "$BOOK_DIR/llms-full.txt"
echo "# pycoati — Full Documentation" >> "$BOOK_DIR/llms-full.txt"
echo "" >> "$BOOK_DIR/llms-full.txt"

# Concatenate all .md files from SUMMARY.md in order
grep -oP '\(([^)]+\.md)\)' "$SRC_DIR/SUMMARY.md" | tr -d '()' | while IFS= read -r path; do
    if [ -f "$SRC_DIR/$path" ]; then
        render "$SRC_DIR/$path" >> "$BOOK_DIR/llms-full.txt"
        printf '\n---\n\n' >> "$BOOK_DIR/llms-full.txt"
    fi
done

echo "Wrote $BOOK_DIR/llms-full.txt"
