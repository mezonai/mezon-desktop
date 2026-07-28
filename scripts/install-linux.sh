#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${MEZON_UPDATE_URL:-https://cdn.komu.vn/desktop/release/latest/}"
case "$BASE_URL" in */) ;; *) BASE_URL="${BASE_URL}/" ;; esac

for tool in curl tar openssl; do
  command -v "$tool" >/dev/null 2>&1 || { echo "error: '$tool' is required" >&2; exit 1; }
done

arch="$(uname -m)"
case "$arch" in
  x86_64 | aarch64) ;;
  *)
    echo "error: unsupported architecture: $arch" >&2
    exit 1
    ;;
esac

manifest_url="${BASE_URL}latest-native-linux-${arch}.yml"
echo "==> Fetching manifest ${manifest_url}"
manifest="$(curl -fsSL "$manifest_url")"

field() { printf '%s\n' "$manifest" | sed -n "s/^${1}:[[:space:]]*//p" | head -1 | tr -d "'\""; }
version="$(field version)"
path="$(field path)"
sha512="$(field sha512)"
[[ -n "$version" && -n "$path" && -n "$sha512" ]] || {
  echo "error: malformed manifest at ${manifest_url}" >&2
  exit 1
}

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
archive="${tmp}/${path##*/}"

echo "==> Downloading Mezon ${version}"
curl -fL --progress-bar -o "$archive" "${BASE_URL}${path}"

echo "==> Verifying checksum"
actual="$(openssl dgst -sha512 -binary "$archive" | openssl base64 -A)"
if [[ "$actual" != "$sha512" ]]; then
  echo "error: checksum mismatch (expected ${sha512}, got ${actual})" >&2
  exit 1
fi

app_dir="${HOME}/.local/share/mezon"
bin_dir="${HOME}/.local/bin"
mkdir -p "$app_dir" "$bin_dir" \
  "${HOME}/.local/share/applications" \
  "${HOME}/.local/share/icons/hicolor/256x256/apps"

tar -xzf "$archive" -C "$tmp"
[[ -f "${tmp}/mezon" ]] || {
  echo "error: archive does not contain the 'mezon' binary" >&2
  exit 1
}

install -m 755 "${tmp}/mezon" "${app_dir}/mezon.new"
mv -f "${app_dir}/mezon.new" "${app_dir}/mezon"
ln -sf "${app_dir}/mezon" "${bin_dir}/mezon"

if [[ -f "${tmp}/mezon.png" ]]; then
  cp "${tmp}/mezon.png" "${HOME}/.local/share/icons/hicolor/256x256/apps/mezon.png"
fi
if [[ -f "${tmp}/mezon.desktop" ]]; then
  sed -e "s|^Exec=.*|Exec=${app_dir}/mezon %u|" "${tmp}/mezon.desktop" \
    >"${HOME}/.local/share/applications/mezon.desktop"
fi

command -v update-desktop-database >/dev/null 2>&1 &&
  update-desktop-database "${HOME}/.local/share/applications" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null 2>&1 &&
  gtk-update-icon-cache -q "${HOME}/.local/share/icons/hicolor" 2>/dev/null || true

echo "==> Installed Mezon ${version} to ${app_dir}/mezon"
case ":$PATH:" in
  *":${bin_dir}:"*) echo "Run: mezon" ;;
  *) echo "Note: ${bin_dir} is not in PATH; run ${app_dir}/mezon or add it to PATH" ;;
esac
