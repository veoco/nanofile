---
description: What the sandbox means for thumbnails, search, people tags and OCR.
---
# Sandbox

**Who this is for:** operators who need to know what the sandbox does to search, thumbnails and the optional media features. **Before you start:** an administrator account.

Everything that parses content a user supplied runs in a separate process with a
memory bound, a CPU bound and a wall-clock timeout, never inside the server:

- document text extraction for the search index,
- image thumbnails,
- media thumbnails (through `ffmpeg`),
- EXIF and avatar processing,
- face detection for the automatic people tags,
- text recognition for the full-text index, for images and scanned PDF pages (`[ocr]`).

The child bounds and confines itself before it reads a request and prints what it
established. The server decides from that report, not from what it asked for.

## What it protects

The resource bound is the one mandatory item for every profile. For the parser
profiles the file, network and process layers are defense in depth against a parser
bug. The media profile is the exception: it starts an external program (`ffmpeg`), so
it runs only when the file and network layers are both in place — without them media
thumbnails are disabled while search, image thumbnails and avatars keep working.
What it does not protect is never asserted to hold: kernel bugs, and other processes
running as the same user. The per-platform mechanisms and the probe are in
[Sandbox internals](/dev/sandbox).

## What it means for you

- **Media thumbnails need `ffmpeg`.** Its path is `storage.ffmpeg_path`; saving it
  re-resolves that grant at once, so the next media request runs the binary you
  just configured. People tags and OCR on HEIC/HEIF/AVIF photos use the same
  helper to decode the image first.
- **On a host that cannot confine the helper, media thumbnails are off.** The media
  profile is the only one that executes an external program, so it requires the file
  and network protections, not just the resource bound. A host that refuses them —
  typically a locked-down Windows host whose container cannot be created — still
  runs search, image thumbnails, avatars, people tags and OCR. The Sandbox page's
  media band shows the missing layer; the uploads themselves are unaffected, they
  just keep their file-type icon instead of a frame.
- **With the sandbox off, these features do not run**, and they do not fall back to
  parsing inside the server process. Documents skipped while it was off are
  recorded as retryable and re-indexed once it is back on — so an outage leaves no
  permanently unsearchable file behind.
- **To see what this host actually gives, read the admin "Sandbox" page**: four
  bands — documents, media, faces and OCR — each with one row per protection and a
  state badge. The images profile (image decoding, EXIF and avatars) has no band of
  its own. The people tag band is measured only while the feature is on; with it
  off the band says so and points at its switch, because the detector's probe runs
  a forward pass.
  The OCR band is measured only while both the index and `[ocr] enabled` are on;
  it loads four models, so a feature nobody enabled does not pay for its probe.
  Each row says what the item is, whether it is in place, how much it matters and
  what its absence opens; the **resource bound** is marked required. Nothing is
  summarised above them, and there is no grade — the rows are the whole verdict.
- **Some items cannot be established on your platform at all**, which is inherent
  to the platform rather than a misconfiguration: the macOS parser profile refuses
  `fork`; Windows has a Job Object with no equivalent for descriptors or file size;
  and the media worker has to start a program by design, so "start programs" is
  recorded as **not applicable** for it. The page notes each one where it bites.

## The master switch

`[sandbox] enabled` is the master switch. With it off:

- none of the features above run,
- nothing falls back to parsing inside the server process,
- documents skipped during that time are recorded as retryable rather than as a
  verdict on their bytes, so they are re-indexed once it is on again.

## Verify

- Open **System Settings → Sandbox**; the resource bound row is marked required and
  in place.
- Each enabled profile shows a state badge; a disabled feature points at its switch.
- If the media band is unavailable, this host cannot give the media profile its file
  and network protections, so media thumbnails are off; the band's reason names the
  missing layer. Search, image thumbnails and avatars are unaffected.
- Upload a document and a thumbnail; both work, or the page names the missing
  mechanism.

## Related

- [Maintenance](/guide/operate/maintenance) — where to look when
  thumbnails or search do not work.
- [Sandbox internals](/dev/sandbox) — what each profile establishes, the per-
  platform residue, and the probe.
- [Configuration](/guide/configure/configuration) — the sandbox settings page.
