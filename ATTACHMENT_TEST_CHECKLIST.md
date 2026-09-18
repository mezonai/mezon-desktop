# Attachment send/receive — test checklist

Everything below is driven through the MCP control socket (desktop) and the
browser (web), against **CLAN TEST 01 → #general**
(`clan 2048775830980530176`, `channel 2048775831257354240`).

A run is only meaningful with **two clients signed in as different accounts** —
one is the sender, the other the receiver, and the interesting behaviour is
almost always on the side that did *not* send.

---

## 0. Setup

### Desktop

```bash
cd <worktree>
cargo build --bin mezon
mkdir -p /tmp/mz2
TMPDIR=/tmp/mz2 RUST_LOG=img_render=info,mezon_store=debug,mezon=info \
  ./target/debug/mezon > /tmp/mz2/run.log 2>&1 &
TMPDIR=/tmp/mz2 ./target/debug/mezon mcp call open_channel \
  --args '{"clan_id":"2048775830980530176","channel_id":"2048775831257354240"}'
```

- [ ] `TMPDIR` is short. The control socket lives under it and `sun_path` is
      104 bytes — a long scratch path silently loses the socket.
- [ ] A second instance needs a *different* `TMPDIR` (e.g. `/tmp/mz3`), otherwise
      the single-instance guard kills it and MCP answers from the old process.
- [ ] `RUST_LOG` includes `img_render` only if the build carries the temporary
      render instrumentation (see §7). `mezon_store=debug` is what prints
      `presign probe` and `presign gate`.

### Web

```bash
npm run dev:chat            # never `yarn` — corepack gives Yarn 4, the repo lock is v1
```

- [ ] `apps/chat/.env` is the intended one (prod vs dev decides which backend a
      pasted session token is valid for). Vite bakes it at **startup** — changing
      `.env` needs a restart.
- [ ] DevTools console filtered to `img-render`.

### Traps that cost the most time

- [ ] **The desktop window must be visible.** An occluded window stops rendering,
      GPUI drops view entities, and every UI tool answers
      `no composer is mounted` / `no topic panel is mounted`. Bring it to the
      front, then re-check `topic_state.panel_mounted` — `panel_open` alone is
      the store's flag and stays `true` after the view is gone.
- [ ] **Staging a dropped file is asynchronous.** `composer_drop_paths` /
      `topic_drop_paths` return before the file is read. Poll
      `composer_state.attachments` / `topic_state.attachments` until the name
      appears; submitting earlier sends the message with **no attachment** and no
      error anywhere.
- [ ] The web page has more than one file input when the topic panel is open —
      uploading to the wrong one puts the file in the other composer.

---

## 1. Direction × receiver state

For each: the sender's row must render from its **local copy** and issue **zero**
proxy requests for the object it just uploaded; the receiver's row goes through
imgproxy.

- [ ] Desktop → web, web **has the channel open**
- [ ] Web → desktop, desktop **has the channel open**
- [ ] Desktop → web, web **in another channel** → 0 requests while away, renders
      after switching back
- [ ] Web → desktop, desktop **in another channel** → same
- [ ] Web → desktop, desktop **closed**, reopened afterwards → backlog renders,
      every attachment clean
- [ ] After switching away and back, the sender's own rows still render locally

Check on the sender:

```bash
grep img_render /tmp/mz2/run.log | grep -c "<object-id>"   # must be 0
```

---

## 2. Topic

The topic reply has no optimistic row — it is built from the server echo — so it
regresses independently of the channel path. Test it separately, every time.

- [ ] Single image into a topic → `kind="local-file"`
- [ ] **Album (3 images) into a topic** → three `local-file` lines. This is the
      case that exercises the name-matched `local_source` guard
- [ ] Album into a topic where the **socket echo beats the ack** (the common case
      for a batch): the paths land on the row the socket already created, and the
      topic list has to be told — check the rendered row, not the store, since a
      store that is right behind a stale view looks like a pass
- [ ] Image + document together → both settle, only the image renders locally
- [ ] Receiver sees the topic reply and its reply-count badge on the parent

---

## 3. File types

Not "does it upload" — every type below takes a **different branch**, and the
branches are what break. Send them in batches and check the row, not just the
state flags.

### Renders inline

| File | Path it exercises | Row must show |
|---|---|---|
| `png` `jpg` | static decode, capped to 1024 px | the picture |
| `gif` | animation path (`message_path_maybe_animated`, frame budget) | plays, does not freeze on frame 1 |
| `webp` | animation path too — animated webp is decoded frame by frame | the picture |
| `heic` | ImageIO on macOS; the `image` crate cannot read it | the picture (verified on macOS) |
| `mp4` | platform demuxer + generated poster | poster, then plays on click |
| `mp3` | audio player | duration and a play control |
| `webm` **audio** (voice message) | symphonia in-app — deliberately exempt from the Matroska block | plays |

### Must fall back to a named file box, never a broken tile

| File | Why |
|---|---|
| `bmp` `tiff` `psd` | in both blocklists; imgproxy cannot read them |
| `avi` `wmv` `flv` `mkv` `rmvb` | in both blocklists |
| `svg` | web excludes `svg+xml` from images on purpose |
| `webm` **video** on macOS/Windows | AVFoundation and Media Foundation have no Matroska demuxer |
| `wma` `ra` | blocked audio |

- [ ] Renders-inline set: all settle to `uploading=0 presign_pending=0 upload_failed=0`
- [ ] Fallback set: every one shows a file box with name and size — no broken
      image, no play button that does nothing
- [ ] Every object on the CDN carries its **real** Content-Type:
      ```bash
      curl -sSI "<url>" | grep -i content-type
      ```
      `image/png`, `image/jpeg`, `image/gif`, `image/bmp`, `image/heic`,
      `image/svg+xml`, `audio/mpeg`, `video/mp4`, `video/webm`,
      `application/pdf`, `text/plain`
- [ ] A type the sender cannot identify uploads as `application/octet-stream`
      (measured for `.avi`) — the row still has to be a file box
- [ ] The extra `.jpg` object with no matching filename is the video poster the
      desktop generates; it has no local file, so one proxy request for it is
      expected
- [ ] Documents show a named box **while uploading**, not an empty row

### Names and sizes

- [ ] A non-ASCII filename (`ảnh-tiếng-việt.png`) uploads and fetches back
      — `sanitize_upload_filename` rewrites the object key, the display name keeps
      its accents
- [ ] A zero-byte file
- [ ] A name long enough to wrap the file box

### Still untested — carry these forward

- [ ] `webp` (macOS `sips` cannot write one; produce it another way)
- [ ] `avif` — known undecodable on desktop for avatars, unverified for messages
- [ ] `mov` / QuickTime — the web has a probe path specifically for it
- [ ] PDF preview on web (`PDFViewerModal`), not just the download box
- [ ] A Tenor GIF — a url attachment, never uploaded, different code path
- [ ] Voice message recorded in-app, as opposed to a `.webm` picked from disk

---

## 4. Failure and recovery

- [ ] **Upload fails** — web: patch `fetch` to reject `PUT` to `/r2-upload`;
      desktop: delete the file right after `composer_submit`
  - [ ] Desktop sender marks `upload_failed` and shows the error overlay
  - [ ] Receiver shows the box with "Uploading…" and makes **no** CDN request
  - [ ] Exactly one PUT attempt — no retry storm
- [ ] **CDN probe** on the receiver backs off `8s → 15s → 30s` then holds, one
      request per round:
      ```bash
      grep "presign probe" /tmp/mz2/run.log
      ```
- [ ] **Expiry** at 10 minutes: the attachment is dropped from the row **in the
      running session**, and the probe stops with it (count stops growing).
      Verifying this after an app restart proves nothing — a fresh load gates
      attachments from the API payload and hides the bug.
- [ ] Send, then leave the channel immediately → upload and the `presign_finish`
      patch still complete; the row is clean on return
- [ ] Two sends back to back → both land, in order, each with its own attachment

---

## 5. Opening the viewer, pressing play

A tile is inert until its bytes are up. The row gates the click on
`!sending && !upload_failed && !presign_pending && !url.is_empty()`
(`parts.rs`), and `open_image_viewer` refuses on the same conditions — a tool
that opened a viewer the user cannot open would report a pass for nothing.

Image viewer:

- [ ] Sender clicks their **own image while it is uploading** → nothing opens
      (`open_image_viewer` answers `still uploading`)
- [ ] Sender clicks after it settles → viewer opens on the CDN url
- [ ] Receiver clicks → viewer opens, and paging walks the channel's other media
- [ ] A failed upload's tile does not open the viewer
- [ ] Web: `canOpenViewer` checks only `isPresignPending` — confirm a row that is
      merely *sending* is inert there too

Video:

- [ ] Sender presses play while uploading → nothing happens
- [ ] Sender presses play after it settles → plays
- [ ] Receiver presses play: `readyState=4`, `error=none`, `currentTime`
      advances. The video streams straight from the CDN; only the poster goes
      through imgproxy

---

## 6. Other paths

- [ ] Reply carrying an attachment (`reply_begin` → drop → `composer_submit`):
      the reply reference survives the presign patch — check the *receiver's* UI
- [ ] Large file — desktop ≥16 MB takes the multipart path (etag ends `-N`);
      web ~9 MB single PUT
- [ ] Reload the page: the sender's local preview is gone, so the row must fall
      back to the proxy and still render

---

## 7. Reading the numbers

The `img_render` lines come from a temporary instrumentation in
`MessageImageLoader` (`crates/mezon-ui/src/image_cache.rs`) — **not committed**,
and it must stay that way: it logs the absolute local path and the full remote
url of every image the app loads, on the hot path. Add it for a measuring run,
then `git checkout origin/develop -- crates/mezon-ui/src/image_cache.rs` before
committing anything.

Healthy ranges measured on a working build:

| | source | time |
|---|---|---|
| Sender, own image | `local-file` | 4–150 ms (capped to 1024 px) |
| Receiver, warm rendition | `imgproxy` | 20–160 ms |
| Receiver, first fetch of a new rendition | `imgproxy` | 500–1600 ms |

- [ ] Sender rows never say `imgproxy` for their own object
- [ ] Each client requests its own `rs:fill:W:H` — a rendition broken for one
      client can be fine on another, so reproduce on the client that reported it

---

## 8. Known open issues to check against

- [ ] Web keeps showing "Uploading…" long past the 10-minute expiry for an
      attachment whose upload died (suspect: `getMessageCreateTimeSeconds`
      returning nothing for a realtime message, so
      `filterExpiredPresignAttachments` bails)
- [ ] A failed upload on web leaves the **sender** on "Uploading…" with no error
      state; the desktop shows a failure
- [ ] imgproxy 5xx responses are cached by Cloudflare with
      `public, max-age=604800` — one transient timeout breaks a rendition for a
      week. Infrastructure fix; clients can only avoid triggering it
