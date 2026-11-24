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
- `ocr` — PP-OCRv6 tiny text detection and recognition plus the PP-LCNet page
  and text-line orientation classifiers, for images and PDF pages in the
  full-text index. The same shape as `faces`, with the widest budget: four
  models, one detection pass and one recognition pass per text line. A PDF
  request also carries the page numbers to render with `hayro`.

## Worker protocol

- **Request** on stdin: `NFX1`, one byte naming the plan, then the file's bytes to
  end of file. The model jobs frame their own requests: faces sends an operation
  byte and a little-endian `u32` options word before the image; OCR does the same
  for an image. A PDF OCR request adds a `u32` page count and that many `u32`
  page numbers before the PDF bytes.
- **Reply** on stdout: one tag byte, a little-endian `u32` length, then that many
  bytes of body — the frame is the same for every job, and the tag set depends on
  the profile. `documents` answers `T` (text) or `U` (not indexable); `ocr` uses
  `T`/`U` too and adds `E` for an environment/model failure, which the parent
  records as retryable rather than as a verdict on the input; `images` answers
  `P`/`J` (a PNG/JPEG thumbnail), `X` (EXIF text) or `U`; `faces` answers `J` (a
  JSON summary), `R` (refused) or `U` (retry); `media` answers `P`/`J`, `E` or
  `U`. A PDF OCR reply separates one field per requested page with `\u{1e}`.
- `--selftest` replaces the request with a report line and no reply.
- `--probe` runs the probe a serving process runs at startup and exits non-zero
  when the worker is unavailable.

The parent's `accept` checks the report before treating the child as confined. The
report carries what the child established, not what the parent asked for. For the
`media` profile that check is stricter: it also requires the `files` and `network`
items, because that profile starts an external program.

## What each profile establishes

| Item | What it is | Linux | macOS | Windows |
|---|---|---|---|---|
| limits | committed memory, CPU seconds; plus descriptors and file size on unix | `RLIMIT_AS` | `setrlimit` (mapped space plus the cap) | Job Object memory and CPU cap |
| files | reading or writing any path | Landlock, zero grants (the helper, its interpreter and the source for media) | Seatbelt profile | AppContainer |
| network | creating a socket | seccomp denylist, Landlock TCP rights | Seatbelt profile | AppContainer with no capabilities |
| process | `exec`, `fork`, extra processes | seccomp denylist (`exec` allowed only for the media helper and its loader) | Seatbelt profile: `exec` only for the media profile, `fork` and `exec` denied for a parser | Job active-process limit, plus the kernel's child-process policy |

`limits` is the hard invariant. A child that cannot establish it is refused by both
sides — the parent's `accept` and the child's own check before it reads a request.

The layers above it are what each profile's job needs:

- Every parser profile — `documents`, `images`, `faces`, `ocr` — runs on `limits`
  alone. `files`, `network` and `process` are defense in depth there: where a
  platform cannot supply one, the feature runs anyway and the report says so.
- `media` runs only with **both** `files` and `network` in place. It is the one
  profile that executes an external program, so without the layers that narrow that
  program to the helper and its source, and without the network denial that keeps it
  from being a general-purpose one, the profile is refused rather than run
  unconfined. Where the host cannot give them (a locked-down Windows host whose
  container cannot be created) media thumbnails are off; the parser profiles keep
  working on the resource bound alone.

The media profile cannot have the process item: it exists to start a program, and
every platform starts one by copying the process first. What bounds the helper is
the files layer and the parent's timeout.

`faces` and `ocr` are the profiles that run models. Faces has a 90 s wall clock and
can split a large photo into nine forward passes; OCR has 180 s, loads four models
once and runs one forward pass for page orientation, detection and each text line.
A PDF request returns the pages read so far at its 150 s soft deadline, inside that
wall clock. The inference runtime is single-threaded, so CPU tracks wall clock
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
  restricted token and process mitigations. A host that confines every command it
  runs refuses the container, and the launch then reports `files=open`: the parser
  profiles keep running on the Job Object alone, while the media profile is
  refused and media thumbnails are off there. See
  [Windows confinement](#windows-confinement) for the mechanisms and the fallback.

The media helper gets its own file grant: the helper and its loader, the libraries
beside it (one file at a time), the system library directories, and — on macOS,
only when the helper actually lives there — the package-manager tree it is
installed under. On Windows the helper's standard streams are pipes rather than
the null device, because a container may refuse to open that device.

## Windows confinement

Windows confinement is a Job Object around the child, a restricted token for it to
run under, an AppContainer around that, and the process mitigations the kernel
will take for it.

Windows has no `RLIMIT_AS` and no unprivileged Landlock, so these are the layers,
and the parent applies every one at creation: a token, a job and a container are
each a property of the process, not something a running one can adopt.

* **The Job Object** (the parent, named as a creation attribute) caps committed
  memory, CPU time and the process count, and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`
  on a handle the parent holds takes whatever the child started with it. The
  child reads the limits back instead of trusting them and holds no handle of
  its own, so nothing inside can raise them. It carries no window-station,
  clipboard or handle restrictions: those apply from the first instruction and
  `JOB_OBJECT_UILIMIT_HANDLES` kills the loader before it can print. There is no
  descriptor or file-size bound here at all.
* **The restricted token** (the parent, at creation) cannot be applied to a
  running process, so the parent starts the child with it: `CreateProcessAsUser`
  with a restricted version of its own token, the one case Windows allows
  without `SeAssignPrimaryToken`. Privileges are gone, the administrative SIDs
  are deny-only, and writes are checked against the restricting SIDs. No
  integrity label is set: low integrity is the container's own property, and the
  child reports the level it reads back (`il=`).
* **The AppContainer** (the parent, at creation) is a process-creation
  attribute: `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES` with an AppContainer
  SID and *no capabilities*. Capabilities are what grant network access and
  access outside the package, so an empty list opens no socket and reads no file
  whose ACL names only the user. This is Chromium's zero-capability
  configuration.

On top, the child sets the process mitigations it can live with — win32k
lockdown, arbitrary code guard, no extension points, no fonts, no remote or
low-integrity images, the ASLR family, strict handle checks, and the
child-process policy for every profile but media — and reads each back with
`GetProcessMitigationPolicy`. They are *runtime* policies, not creation
attributes, because the media profile must start its helper. Between creation
and `confine` the job's active-process limit is the only bound on child
creation, and nothing untrusted runs in that window.

The restricting SIDs are this process's own identity: the logon session, the
user, and the groups Windows grants the objects a process needs to attach to
(`INTERACTIVE`, `Authenticated Users`, `Users`, `Everyone`). They cannot be
dropped: a restricted token is checked twice on every access, and a restricting
list without them leaves the child created and dead inside the loader with
`STATUS_DLL_INIT_FAILED` (`0xC0000142`).

Because the user's own SID is in that list, `WRITE_RESTRICTED` narrows writes
to what this user may write rather than to nothing, and is close to a no-op on
its own. What bounds writes is the container and the integrity level: an
AppContainer check requires an ACE for the package SID — or for
`ALL APPLICATION PACKAGES` — *in addition* to what the user SIDs grant, so a
file naming only the user stops being readable. The worker's own image is the
one file that has to stay readable and a per-user install carries no such ACE,
so the parent grants it to the container before the first launch
(`grant_image_access`).

With the container, all four items are the platform's own. Without it the child
still runs in the job, under the restricted token and with the mitigations, and
reports `files=open`: `limits` is what keeps the server up, and the container is
the only mechanism that could add the files and network items. Every launch
attempts it once; a host that refuses is not asked twice.

### Why a host may end up without the container

Creating one is a registry profile plus package-SID access on the worker's own
image, and a host sandbox that confines every command it runs — a development
or agent environment — refuses exactly those. The failure arrives as a child
that dies in its loader with no diagnosis, or as an ACL that cannot be written.
A launch therefore attempts the container first and falls back to the job
object, reporting the missing item.

### The less privileged container

Windows 11 lets a container opt out of `ALL APPLICATION PACKAGES` with
`PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY` `..._OPT_OUT`, and
`lpac=` says whether it took. Windows 10 answers with an error, so the attribute
is dropped and the plain container kept. The attribute is accepted at creation,
so a container the loader cannot start in fails *after* `CreateProcess` returns
success; the parent then stops asking and starts over in the plain container.

The opt-out does not close the note below. It is the security attribute
`WIN://NOALLAPPPKG`, which makes the low-box check *ignore* `ALL APPLICATION
PACKAGES` without removing it from the token, and the process stays an
AppContainer. What keeps the system tree readable is another principal,
`ALL RESTRICTED APPLICATION PACKAGES` (`S-1-15-2-2`), whose ACEs ship on the
system files a packaged process loads — a property of the Windows build rather
than a documented invariant, which is why `system=` is measured and why a host
answering `system=denied` is stricter rather than broken. Nor does the opt-out
cover the registry: an LPAC needs the `registryRead` capability to read the keys
a parser would want.

What no protection here bounds is the shape of the boundary. The container reads
the system tree it loads from (`Windows`, `Program Files` — the trees whose ACLs
carry the packages a low-box process is checked against), reported as
`system=readable` beside the per-user paths it was refused. A write is reported
as measured — `writes=store` inside the container's redirected store,
`writes=user` outside it — and writes have no size bound here at all. The
registry it reads and the IPC it reaches are the same dual-principal check and
the handles this process handed it: the platform's own limits, the same shape
as the macOS profile's grants.

### Where the media profile's helper may live

The helper needs an explicit grant — on its own file and on the libraries
beside it, one file at a time, and nothing else — because a container's file
access is decided by ACEs naming its own package SID. The child reports
`helper=allowed` or `helper-grant-refused`.

Granting only files is deliberate. Walking *up* and granting every directory
would be unnecessary: `restricted_token` keeps `SeChangeNotifyPrivilege`
(`DISABLE_MAX_PRIVILEGE` disables everything except it), so a caller holding it
is not checked for traverse at all. It is also ruinous: `SetNamedSecurityInfo`
"impose[s] the current inheritance model on the ACLs of all objects in the
hierarchy below the target object" (Microsoft, *Automatic Propagation of
Inheritable ACEs*), so one ACE on `C:\Users\<user>` rewrites the inheritance of
everything beneath it.

A helper's stdio is the first suspect when it cannot run: `NUL` is a device a
container may refuse to open, and that refusal arrives from the same call as the
process creation. The streams are pipes — stdin closed here, stdout and stderr
drained — and the launch reports `parse=media-ok(<bytes>)`. A helper whose only
grant is `ALL APPLICATION PACKAGES` gets the opt-out given up for the media
profile rather than an environment problem reported (`use_plain_container`).

### The two principals, and why the opt-out is the one that can go

A low-box check is answered by an ACE for the process's package SID *or* for
`ALL APPLICATION PACKAGES`; the second is what the system tree carries, and the
shortcut `dacl_grants_any_package` takes it so a file the OS already granted
is not rewritten on every launch. The opt-out makes the check ignore it, so that
shortcut certifies a read only while the launch is not asking for the opt-out.
`add_access` therefore asks from the launch's own decision, and a file this
process may not re-DACL — a system-owned DLL, an image under `Program Files` —
is where the two meet: the opt-out is given up and the container kept, rather
than the child being handed an image it cannot read. The launch reports
`lpac=off`, the settings page carries the note, and the log says the media
profile gave it up.

### References

* Microsoft, *Launch an AppContainer*: the attribute, the empty capability
  list, and the requirement that the image be readable by the container.
* Microsoft, *Process Mitigation Policies* and *Job Objects*, including
  `PROC_THREAD_ATTRIBUTE_JOB_LIST` and the child-process policy that is only
  accepted for a process in a job.
* Chromium's `sandbox/win/src/`: the same launch, the same one-active-process
  job, and the note that the creation-time child-process policy exists
  *because* a job can be escaped.

## Tests

The sandbox is tested where it is used, not only where it is described.

- `server/tests/sandbox_probe_test.rs` asserts each profile's typed report and the
  platform's own mechanism tokens beside it.
- Each profile also has to do its job under the confinement it claims, judged by
  its product: the probe parses every structured format it carries and compares
  the whole text each parser returns; the images child's reply is checked against
  the pixels it was given; the media child reads a file the parent wrote, decodes a
  frame and hands back an encoded thumbnail; the faces child loads the model and
  runs one detection, reporting `faces-ok(0)`; the OCR child loads all four
  models, runs blank page and line orientation classifications, a blank
  detection and a blank recognition, and reports `ocr-ok(0)` (that it reads real text is the OCR
  engine's unit test and the index integration test's subject).
- A document whose text is near the size cap is extracted whole in the confined
  child, so the resource limits are measured as sufficient rather than merely
  installed.
- A host where the worker cannot run at all has to refuse an avatar, an EXIF read
  and a thumbnail rather than decode them in the server process.
- The media profile's extra requirement is tested from both sides: a synthetic
  report missing `files` or `network` is refused with the layer named, and a host
  that refuses the layers makes the media tests skip on that reason rather than
  fail. The parser profiles on the same host are still measured and still run.

The released binary and the published image run the same probe, which is the only
place the sandbox runs in a `FROM scratch` container. See
[CI and releases](/dev/ci-releases).

## Limitations

What the sandbox does not defend against is not asserted to hold: a kernel
vulnerability, and another process running as the same user. The tests are about
the confinement this server builds, not about the kernel underneath it.
