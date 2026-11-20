#!/usr/bin/env bash
# The mdBook toolchain the docs are built with — the single source of truth for
# both `make docs` and `.github/workflows/docs.yml`.
#
# The pins matter because mdbook and mdbook-mermaid share a preprocessor
# protocol that changed between mdbook 0.4 and 0.5. A mismatched pair does not
# say so: mdbook-mermaid fails to deserialise the book it is handed and reports
#
#     Unable to parse the input
#     Error: The "mermaid" preprocessor exited unsuccessfully with exit status: 1
#
# which names neither tool's version. Hence the check below.
#
# Usage:
#   bash docs/toolchain.sh             # verify the installed versions
#   eval "$(bash docs/toolchain.sh --versions)"   # export the pins for CI
set -euo pipefail

MDBOOK_VERSION="0.5.2"
MDBOOK_MERMAID_VERSION="0.17.0"

if [[ "${1:-}" == "--versions" ]]; then
    echo "MDBOOK_VERSION=$MDBOOK_VERSION"
    echo "MDBOOK_MERMAID_VERSION=$MDBOOK_MERMAID_VERSION"
    exit 0
fi

fail=0

# `mdbook v0.5.2` / `mdbook-mermaid 0.17.0` — take the first dotted number.
installed_version() {
    "$1" --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1
}

# Patch releases are compatible; the protocol travels with the minor series.
series() {
    cut -d. -f1,2 <<< "$1"
}

check() {
    local bin="$1" pinned="$2" hint="$3" found
    if ! command -v "$bin" > /dev/null 2>&1; then
        echo "❌ $bin is not on PATH — install it with:" >&2
        echo "     $hint" >&2
        fail=1
        return
    fi

    found="$(installed_version "$bin")"
    if [[ -z "$found" ]]; then
        echo "⚠️  could not read a version from \`$bin --version\`; continuing" >&2
        return
    fi
    if [[ "$(series "$found")" != "$(series "$pinned")" ]]; then
        echo "❌ $bin $found does not match the pinned $pinned series." >&2
        echo "   The two docs tools share a preprocessor protocol, and a" >&2
        echo "   mismatched pair fails with an error that names neither." >&2
        echo "   Fix with:" >&2
        echo "     $hint" >&2
        fail=1
    fi
}

check mdbook "$MDBOOK_VERSION" \
    "cargo install mdbook --version $MDBOOK_VERSION --locked"
check mdbook-mermaid "$MDBOOK_MERMAID_VERSION" \
    "cargo install mdbook-mermaid --version $MDBOOK_MERMAID_VERSION --locked"

exit "$fail"
