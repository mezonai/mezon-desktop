# libwebrtc builds

`crates/vendor/webrtc-sys` links a prebuilt static libwebrtc. The scripts in this directory build it
from [webrtc-sdk/webrtc](https://github.com/webrtc-sdk/webrtc) at the commit pinned in `.gclient`,
with the patches in `patches/`.

## Publishing a build

1. Commit any change to this directory, then tag that commit with the value of `WEBRTC_TAG` in
   `crates/vendor/webrtc-sys-build/src/lib.rs` and push the tag:

   ```sh
   git tag webrtc-m144-aaeeee8
   git push origin webrtc-m144-aaeeee8
   ```

2. `.github/workflows/webrtc-builds.yml` builds `win-x64`, `mac-arm64`, `mac-x64`, `linux-x64` and
   `linux-arm64`, and uploads `webrtc-<os>-<arch>-release.zip` to the GitHub release of that tag.
   The release is marked pre-release and never becomes "latest".

`webrtc-sys` downloads
`https://github.com/mezonai/mezon-desktop/releases/download/<WEBRTC_TAG>/webrtc-<os>-<arch>-release.zip`.

Changing the pinned commit or the patches needs a new tag and a matching `WEBRTC_TAG`.

## Building locally

From this directory:

```sh
./build_macos.sh --arch arm64 --profile release
```

Then point the Rust build at the output instead of the download:

```sh
MEZON_CUSTOM_WEBRTC=$PWD/mac-arm64-release cargo build
```

`MEZON_DEBUG_WEBRTC=true` selects the `-debug` artifact instead of `-release`.
