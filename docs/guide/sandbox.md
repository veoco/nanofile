# Sandbox

Everything that parses content a user supplied runs in a separate process with a
memory bound, a CPU bound and a wall-clock timeout, never inside the server:

- document text extraction for the search index,
- image thumbnails,
- media thumbnails (through `ffmpeg`),
- EXIF and avatar processing.

The child bounds and confines itself before it reads a request and prints what it
established. The server decides from that report, not from what it asked for.

## The resource bound is mandatory

The resource bound is the one item no setting may give up. A child that cannot
establish it is refused by both sides, because a parser bomb or an unbounded loop
is the failure this exists to prevent. A host that cannot establish the bound at
all runs none of these features.

Everything else these features parse is pure Rust (`lopdf`, `office_oxide`, `zip`,
`image`, `kamadak-exif`). The one external program any profile starts is the media
helper. The file, network and process layers are therefore deliberate defense in
depth against a parser bug, not the bound that keeps the server up.

## The admin Sandbox page

The **Sandbox** page lists what this host actually gives: one band per profile it
measured, one row per protection, and a state badge on each band saying whether
that profile runs. Nothing is summarised above them — no grade and no verdict
sentence, because both would only restate what the badges and rows already say.

Each row says what the item is, whether it is in place, how much it matters and
what its absence opens:

- **Resource bound** — memory and CPU everywhere, plus descriptors and file size
  on Linux and macOS. Marked as required.
- **File access** — paths cannot be read or written. The item that keeps a parser
  out of the server user's data; the page names its absence rather than treating
  the other layers as substitutes.
- **Network access** and **starting programs** — refinements on top of a
  files-confined host, never replacements for one.

The platform's own residuals are listed beside the item they weaken:

- macOS's parser profiles deny the fork outright and narrow the exec to the
  child's own image.
- The Windows container reads the system tree it loads from; its Job Object bounds
  memory and CPU time where it has no descriptor or file-size bound.
- The media worker is granted the libraries it needs as well as the helper itself.

## The media worker

The media worker has a band of its own, and its list is incomplete by design. It
exists to start a program, and every way of starting one copies the process
first, so its process item cannot exist. The page says **Not applicable** rather
than reporting a gap. What bounds it instead is the file layer and the time
limits, which bound the copies.

- On Linux and macOS the file layer decides `exec` per path: the helper and the
  interpreter the kernel runs before it, never a directory.
- On Windows the container does not restrict which readable program may run, so
  only the Job Object's process count and the timeout bound a compromised helper.

Its file item carries its own notes. A helper is a program with libraries: it may
read the libraries beside it (one file at a time), the system library directories,
and — on macOS, and only when the helper actually lives there — the
package-manager tree it is installed under (`/opt/homebrew`, `/usr/local` or
`/opt/local`).

On Linux the media child may not call `setsid`/`setpgid`, so a copy it makes
cannot leave the process group the parent kills. On macOS nothing denies those
calls, so a copy can leave that group; the only bound left is the CPU limit each
copy carries and the parent's timeout — not the number of copies.

## The master switch

`[sandbox] enabled` is the master switch. With it off:

- none of these features run,
- nothing falls back to parsing inside the server process,
- a document skipped while the switch is off is recorded as a retryable failure
  rather than a verdict on its bytes, so turning the switch back on re-indexes it.

Everything above the resource bound is the platform's answer rather than a
setting. The page's rows are measured from confined children, so it cannot
advertise a layer this host does not have.

## Windows

On Windows the container is attempted, and a host that refuses it falls back to
the bound. The AppContainer is the only mechanism that adds the files and network
items there, and creating one needs a registry profile and package access on the
worker's own image.

A host that confines every command it runs — a development or agent environment —
refuses exactly that, and the child dies in its loader with nothing to diagnose.
So a Windows launch that cannot create one applies the Job Object, the restricted
token and the process mitigations and reports `files=open`, and the container is
not asked for again in that process. The page names what is missing either way.

## The media helper

`storage.ffmpeg_path` names the helper the media profile may execute. Saving it
re-resolves the grant through the same hook as the master switch, so the next
media request runs the binary that was just configured.

- On Windows the container is granted the helper's file **and the libraries beside
  it**, never a directory. Its token keeps `SeChangeNotifyPrivilege`, so the walk
  to the file is not checked, while writing a DACL on a directory re-imposes
  inheritance on everything below it.
- The helper's standard streams are pipes rather than the null device, because a
  container may refuse to open that device and the refusal would read as "the
  helper cannot run".
- A helper that can only run in the plain container — its read access coming from
  `ALL APPLICATION PACKAGES`, which the less privileged container makes the check
  ignore — is run there instead. The opt-out is given up (`lpac=off` in the report
  and on the page) and the container kept.
- A container may need the Landlock syscalls allowed for every item to be in
  place.

## What is tested, and what is not defended

The confinement is tested where it is used, not only where it is described. Every
platform this project ships runs the whole Rust suite, and each profile's report
is asserted in one place. The probe parses every structured format it carries,
checks the image child's reply against the pixels it was given, and runs the media
pipeline end to end. A host where the worker cannot run at all has to refuse an
avatar, an EXIF read and a thumbnail rather than decode them in the server
process.

See [Sandbox internals](/dev/sandbox) for the mechanisms and probe.

What the sandbox does not defend against is not asserted to hold: a kernel
vulnerability, and another process running as the same user. The tests are about
the confinement this server builds, not about the kernel underneath it.
