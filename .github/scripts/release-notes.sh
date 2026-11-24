#!/usr/bin/env bash
# Prints the release notes for <version> (e.g. v0.1.0): its CHANGELOG.md section, then how to install it.
set -euo pipefail

version=$1
number=${version#v}

changes=$(awk -v heading="## $version" '
  $0 == heading { found = 1; next }
  found && /^## / { exit }
  found && /^$/ { blanks++; next }
  found { if (printed) for (; blanks; blanks--) print ""; blanks = 0; printed = 1; print }
' CHANGELOG.md)
if [[ -z "$changes" ]]; then
  echo "CHANGELOG.md has no section for $version" >&2
  exit 1
fi

cat <<NOTES
$changes

## Install

\`\`\`sh
pip install epochal-api==$number epochal-sandbox==$number
cargo add epochal-api@$number epochal-sandbox@$number
cargo install epochal-cli@$number epochal-mcp@$number
\`\`\`

Prebuilt \`epochal\` and \`epochal-mcp\` binaries for Linux (x86_64, arm64), macOS (Intel, Apple
silicon) and Windows (x86_64) are attached below. \`SHA256SUMS\` lists their checksums, and
\`gh attestation verify <file> --repo epochal-org/epochal\` checks they were built here.

NOTES
