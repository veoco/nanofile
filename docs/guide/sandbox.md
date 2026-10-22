# Sandbox

Everything that parses content a user supplied runs in a separate process with a
memory bound, a CPU bound and a wall-clock timeout, never inside the server:

- document text extraction for the search index,
- image thumbnails,
- media thumbnails (through `ffmpeg`),
- EXIF and avatar processing.

The child bounds and confines itself before it reads a request and prints what it
established. The server decides from that report, not from what it asked for.

## What it protects

- **The resource bound is the one mandatory item.** A child that cannot establish
  it is refused by both sides — a parser bomb or an unbounded loop is the failure
  this exists to prevent. A host that cannot establish the bound at all runs none
  of these features.
- **The file, network and process layers are defense in depth** against a bug in a
  parser: everything these features parse is pure Rust (`lopdf`, `office_oxide`,
  `zip`, `image`, `kamadak-exif`), and the only external program any profile starts
  is the media helper.
- **What it does not protect** is never asserted to hold: kernel bugs, and other
  processes running as the same user.

Those mechanisms are built per platform, not chosen by a setting — the operating
system decides what can be established.

## What it means for you

- **Media thumbnails need `ffmpeg`.** Its path is `storage.ffmpeg_path`; saving it
  re-resolves that grant at once, so the next media request runs the binary you
  just configured.
- **With the sandbox off, these features do not run**, and they do not fall back to
  parsing inside the server process. Documents skipped while it was off are
  recorded as retryable and re-indexed once it is back on — so an outage leaves no
  permanently unsearchable file behind.
- **To see what this host actually gives, read the admin "Sandbox" page**: one
  band per measured profile, one row per protection, and a state badge per band.
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

## Related

- [Maintenance and troubleshooting](/guide/maintenance) — where to look when
  thumbnails or search do not work.
- [Sandbox internals](/dev/sandbox) — what each profile establishes, the per-
  platform residue, and the probe.
- [Configuration](/guide/configuration) — the sandbox settings page.
