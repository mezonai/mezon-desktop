#!/usr/bin/env bash
set -euo pipefail

artifact="${1:?usage: make-update-manifest.sh <artifact> <version> <out-yml> [deb]}"
version="${2:?missing version}"
out="${3:?missing output path}"
deb="${4:-}"

[[ -f "$artifact" ]] || { echo "artifact not found: $artifact" >&2; exit 1; }

sha512="$(openssl dgst -sha512 -binary "$artifact" | openssl base64 -A)"
size="$(wc -c < "$artifact" | tr -d '[:space:]')"

cat > "$out" <<EOF
version: ${version}
path: $(basename "$artifact")
sha512: ${sha512}
EOF

if [[ -n "$deb" ]]; then
  [[ -f "$deb" ]] || { echo "deb not found: $deb" >&2; exit 1; }
  deb_sha512="$(openssl dgst -sha512 -binary "$deb" | openssl base64 -A)"
  cat >> "$out" <<EOF
deb: $(basename "$deb")
debSha512: ${deb_sha512}
EOF
fi

cat >> "$out" <<EOF
size: ${size}
releaseDate: '$(date -u +%Y-%m-%dT%H:%M:%S.000Z)'
EOF
echo "wrote ${out}"
