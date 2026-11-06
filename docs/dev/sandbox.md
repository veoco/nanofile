# Sandbox internals

Every parser that reads attacker-chosen bytes runs in a separate process: the same
binary re-executed as `nanofile extract-worker`. The child confines itself and only
then reads a request on stdin. There is no in-process path: if the child cannot be
started, or refuses to run without confinement, the document is a failure — retried
later, never parsed by the server.

See [Sandbox](/guide/admin/sandbox) for the operator-facing behaviour and the admin page.

## Profiles

A profile names what the child is allowed to establish:

- `documents` — text extraction for the search index.
- `images` — image decode and EXIF/avatar processing.
- `media` — `ffmpeg` thumbnails, the one profile that starts an external program.
- `faces` — face detection for the automatic people tags. The same shape as
  `images`, with a much wider budget: the detector is a model profile.
- `ocr` — PP-OCRv6 tiny text detection and recognition for the full-text index.
  The same shape as `faces`, with the widest budget: two models, one detection
  pass and one recognition pass per text line.

## Worker protocol

- **Request** on stdin: `NFX1`, one byte naming the plan, then the file's bytes to
  end of file. The model jobs frame their own requests: faces sends an operation
  byte and a little-endian `u32` options word before the image; OCR does the same.
- **Reply** on stdout: `T` or `U`, a little-endian `u32` length, then that many bytes
  of UTF-8 — the text, or the reason the document is not indexable. OCR adds `E`
  for an environment/model failure, which the parent records as retryable rather
  than as a verdict on the image.
- `--selftest` replaces the request with a report line and no reply.
- `--probe` runs the probe a serving process runs at startup and exits non-zero
  when the worker is unavailable.

The parent's `accept` checks the report before treating the child as confined. The
report carries what the child established, not what the parent asked for.

## What each profile establishes

| Item | What it is | Linux | macOS | Windows |
|---|---|---|---|---|
| limits | committed memory, CPU seconds; plus descriptors and file size on unix | `RLIMIT_AS` | `setrlimit` (mapped space plus the cap) | Job Object memory and CPU cap |
| files | reading or writing any path | Landlock, zero grants (the helper, its interpreter and the source for media) | Seatbelt profile | AppContainer |
| network | creating a socket | seccomp denylist, Landlock TCP rights | Seatbelt profile | AppContainer with no capabilities |
| process | `exec`, `fork`, extra processes | seccomp denylist (`exec` allowed only for the media helper and its loader) | Seatbelt profile: `exec` only for the media profile, `fork` and `exec` denied for a parser | Job active-process limit, plus the kernel's child-process policy |

`limits` is the hard invariant. A child that cannot establish it is refused by both
sides — the parent's `accept` and the child's own check before it reads a request.
`files`, `network` and `process` are defense in depth: where a platform cannot
supply one, the feature runs on `limits` alone and the report says so.

The media profile cannot have the process item: it exists to start a program, and
every platform starts one by copying the process first. What bounds the helper is
the files layer and the parent's timeout.

`faces` and `ocr` are the profiles that run models. Faces has a 90 s wall clock and
can split a large photo into nine forward passes; OCR has 180 s, loads two graphs,
prepares a detection plan for the picture's size and recognition plans for the
widths it meets, then runs one recognition per text line. tract's CPU runtime does
not enable its multithreaded matrix multiply by default, so CPU tracks wall clock
(with 5 s of hard-limit slack); the wall timeout is still the real bound. Neither
starts a program or reads a file: HEIC/HEIF/AVIF frames are decoded by the media
profile first, and only the decoded still is handed to them.

## Platform notes

- **Linux** — Landlock for the file and TCP layers, seccomp for the network and
  process layers, `RLIMIT_AS`/`RLIMIT_CPU`/`RLIMIT_NOFILE`/`RLIMIT_FSIZE` for the
  bound. The refusals are tested against the running kernel: the filter is
  installed in a child that then makes the calls a parser must not be able to make,
  each beside the same call in a process without the filter.
- **macOS** — every child runs under `sandbox-exec` with a Seatbelt profile.
  Parser profiles deny `fork` and narrow `exec` to the child's own image.
- **Windows** — the AppContainer is attempted first, then the Job Object, the
  restricted token and process mitigations. The AppContainer needs a registry
  profile and package access on the worker's own image, and a host that confines
  every command it runs refuses it. In that case the launch reports `files=open`
  and the container is not asked for again in that process. The Job Object bounds
  committed memory and CPU time; it has no descriptor or file-size bound.

The media helper gets its own file grant: the helper and its loader, the libraries
beside it (one file at a time), the system library directories, and — on macOS,
only when the helper actually lives there — the package-manager tree it is
installed under. On Windows the helper's standard streams are pipes rather than
the null device, because a container may refuse to open that device.

## Tests

The sandbox is tested where it is used, not only where it is described.

- `server/tests/sandbox_probe_test.rs` asserts each profile's typed report and the
  platform's own mechanism tokens beside it.
- Each profile also has to do its job under the confinement it claims, judged by
  its product: the probe parses every structured format it carries and compares
  the whole text each parser returns; the images child's reply is checked against
  the pixels it was given; the media child reads a file the parent wrote, decodes a
  frame and hands back an encoded thumbnail; the faces child loads the model and
  runs one detection, reporting `faces-ok(0)`; the OCR child loads both models,
  runs a blank detection and a blank recognition, and reports `ocr-ok(0)` (that
  it reads real text is the OCR engine's unit test and the index integration
  test's subject).
- A document whose text is near the size cap is extracted whole in the confined
  child, so the resource limits are measured as sufficient rather than merely
  installed.
- A host where the worker cannot run at all has to refuse an avatar, an EXIF read
  and a thumbnail rather than decode them in the server process.

The released binary and the published image run the same probe, which is the only
place the sandbox runs in a `FROM scratch` container. See
[CI and releases](/dev/ci-releases).

## Limitations

What the sandbox does not defend against is not asserted to hold: a kernel
vulnerability, and another process running as the same user. The tests are about
the confinement this server builds, not about the kernel underneath it.
