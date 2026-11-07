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

The resource bound is the one mandatory item. The file, network and process layers are
defense in depth against a parser bug. What it does not protect is never asserted to
hold: kernel bugs, and other processes running as the same user. The per-platform
mechanisms and the probe are in [Sandbox internals](/dev/sandbox).

## What it means for you

- **Media thumbnails need `ffmpeg`.** Its path is `storage.ffmpeg_path`; saving it
  re-resolves that grant at once, so the next media request runs the binary you
  just configured. People tags and OCR on HEIC/HEIF/AVIF photos use the same
  helper to decode the image first.
- **With the sandbox off, these features do not run**, and they do not fall back to
  parsing inside the server process. Documents skipped while it was off are
  recorded as retryable and re-indexed once it is back on — so an outage leaves no
  permanently unsearchable file behind.
- **To see what this host actually gives, read the admin "Sandbox" page**: one
  band per profile, one row per protection, and a state badge per band. The people
  tag band is measured only while the feature is on; with it off the band says so
  and points at its switch, because the detector's probe runs a forward pass.
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
- Upload a document and a thumbnail; both work, or the page names the missing
  mechanism.

## Related

- [Maintenance](/guide/operate/maintenance) — where to look when
  thumbnails or search do not work.
- [Sandbox internals](/dev/sandbox) — what each profile establishes, the per-
  platform residue, and the probe.
- [Configuration](/guide/configure/configuration) — the sandbox settings page.
