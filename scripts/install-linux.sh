#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${MEZON_UPDATE_URL:-https://cdn.komu.vn/desktop/release/latest/}"
case "$BASE_URL" in */) ;; *) BASE_URL="${BASE_URL}/" ;; esac

for tool in curl tar openssl; do
  command -v "$tool" >/dev/null 2>&1 || { echo "error: '$tool' is required" >&2; exit 1; }
done

# The tarball holds only the binary: unlike the .deb, nothing here pulls in the
# GStreamer decoders Mezon needs at runtime. Without one the app still starts, but
# videos neither play nor get a thumbnail, and GStreamer reports that only in the
# log -- so install it here, the same way scripts/linux-deps installs build deps,
# and fall back to naming the package if we cannot.
if [ "$(id -u)" -eq 0 ]; then
  maysudo=''
else
  maysudo="$(command -v sudo || true)"
fi

h264_decoder_elements="avdec_h264 openh264dec vah264dec vaapih264dec nvh264dec v4l2h264dec"
h264_decoder_plugins="libgstlibav.so libgstopenh264.so libgstva.so libgstvaapi.so libgstnvcodec.so libgstvideo4linux2.so"

video_decoder_installed() {
  local element dir plugin
  if command -v gst-inspect-1.0 >/dev/null 2>&1; then
    for element in $h264_decoder_elements; do
      if gst-inspect-1.0 "$element" >/dev/null 2>&1; then
        return 0
      fi
    done
  fi
  for dir in /usr/lib/gstreamer-1.0 /usr/lib64/gstreamer-1.0 \
             /usr/lib/*/gstreamer-1.0 /usr/local/lib/gstreamer-1.0; do
    for plugin in $h264_decoder_plugins; do
      if [[ -e "${dir}/${plugin}" ]]; then
        return 0
      fi
    done
  done
  return 1
}

video_decoder_hint() {
  if command -v apt-get >/dev/null 2>&1; then
    echo "sudo apt-get install -y --no-install-recommends gstreamer1.0-libav gstreamer1.0-plugins-good"
  elif command -v dnf >/dev/null 2>&1; then
    echo "sudo dnf install -y gstreamer1-plugin-libav gstreamer1-plugins-good"
  elif command -v pacman >/dev/null 2>&1; then
    echo "sudo pacman -S --needed gst-libav gst-plugins-good"
  elif command -v zypper >/dev/null 2>&1; then
    echo "sudo zypper install -y gstreamer-plugins-libav gstreamer-plugins-good"
  else
    echo "install your distribution's GStreamer libav (or openh264) plugin"
  fi
}

install_video_decoder() {
  if [ "$(id -u)" -ne 0 ] && [ -z "$maysudo" ]; then
    return 1
  fi
  if command -v apt-get >/dev/null 2>&1; then
    if ! $maysudo apt-get update; then
      echo "warning: apt-get update failed; installing against a stale package index" >&2
    fi
    $maysudo apt-get install -y --no-install-recommends \
      gstreamer1.0-libav gstreamer1.0-plugins-good
  elif command -v dnf >/dev/null 2>&1; then
    $maysudo dnf install -y gstreamer1-plugin-libav gstreamer1-plugins-good
  elif command -v pacman >/dev/null 2>&1; then
    $maysudo pacman -S --needed --noconfirm gst-libav gst-plugins-good
  elif command -v zypper >/dev/null 2>&1; then
    $maysudo zypper install -y gstreamer-plugins-libav gstreamer-plugins-good
  else
    return 1
  fi
}

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
icon_root="${HOME}/.local/share/icons/hicolor"
icon_sizes="16x16 24x24 32x32 48x48 64x64 128x128 256x256"
mkdir -p "$app_dir" "$bin_dir" "${HOME}/.local/share/applications"
for size in $icon_sizes; do
  mkdir -p "${icon_root}/${size}/apps"
done

tar -xzf "$archive" -C "$tmp"
[[ -f "${tmp}/mezon" ]] || {
  echo "error: archive does not contain the 'mezon' binary" >&2
  exit 1
}

install -m 755 "${tmp}/mezon" "${app_dir}/mezon.new"
mv -f "${app_dir}/mezon.new" "${app_dir}/mezon"
ln -sf "${app_dir}/mezon" "${bin_dir}/mezon"

if [[ -d "${tmp}/icons" ]]; then
  for size in $icon_sizes; do
    if [[ -f "${tmp}/icons/${size}/mezon.png" ]]; then
      cp "${tmp}/icons/${size}/mezon.png" "${icon_root}/${size}/apps/mezon.png"
    fi
  done
elif [[ -f "${tmp}/mezon.png" ]]; then
  cp "${tmp}/mezon.png" "${icon_root}/256x256/apps/mezon.png"
fi
if [[ -f "${tmp}/mezon.desktop" ]]; then
  sed -e "s|^Exec=.*|Exec=${app_dir}/mezon %u|" "${tmp}/mezon.desktop" \
    >"${HOME}/.local/share/applications/mezon.desktop"
fi

command -v update-desktop-database >/dev/null 2>&1 &&
  update-desktop-database "${HOME}/.local/share/applications" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null 2>&1 &&
  gtk-update-icon-cache -q "${HOME}/.local/share/icons/hicolor" 2>/dev/null || true

if ! video_decoder_installed; then
  echo ""
  echo "==> No GStreamer H.264 decoder found; installing one (videos need it)."
  echo "    This needs root, so you may be asked for your password."
  if install_video_decoder && video_decoder_installed; then
    echo "==> Video decoder installed"
  else
    echo ""
    echo "Warning: could not install a GStreamer H.264 decoder automatically."
    echo "Videos will not play, and videos you send will have no thumbnail, until you run:"
    echo "    $(video_decoder_hint)"
  fi
fi

echo "==> Installed Mezon ${version} to ${app_dir}/mezon"
case ":$PATH:" in
  *":${bin_dir}:"*) echo "Run: mezon" ;;
  *) echo "Note: ${bin_dir} is not in PATH; run ${app_dir}/mezon or add it to PATH" ;;
esac
