---
description: A tour of the Nanofile web interface, including libraries, the file browser, search, trash and settings.
---
# Web interface

Once you are signed in, the file manager is what you see first. This page is a tour
of the frame, the library list and the main browsing areas. For uploads, moves,
versions, sharing and trash actions, see [Files and sharing](/guide/use/files-and-sharing).

**Who this is for:** anybody using the browser interface. **Before you start:** an
account and at least read permission on a library.

## The frame

- **Left rail** — the library tree on top, with its own filter box, and a "Browse"
  block below it: starred, shares, trash, activity. At the bottom, your storage meter
  and the version.
- **Top-bar search box** — Enter opens the search page, see [Search](#search).
- **Details drawer** — rises from the bottom when you select an entry, see
  [The file browser](#the-file-browser).

Administrators also see the **System Settings** entry in the user menu; the admin
console is covered in [Configuration](/guide/configure/configuration) and
[Users and sharing](/guide/admin/users-and-sharing).

## Libraries

`/libraries/` lists the libraries you can reach, sortable by name, modified time or
size, and filterable.

- **New library** sits in the page header. The web interface creates a **regular**
  library; an **encrypted library is created by a client** (it carries a lock badge in
  the list), and its password is entered in the client — the server cannot supply it.
- The **⋯** menu on a row opens **edit**: change the name and description, set
  **history retention** (how many versions to keep, how many days to keep them), and
  see that library's WebDAV address.
- Deleting a library moves it to the "Libraries" tab of the trash; it does not vanish
  on the spot.

## The file browser

`/libraries/<library id>/files/` is the file manager.

### Views

- **List** — name, size, modified time.
- **Grid** — tiles with thumbnails.
- **Gallery** — images and videos grouped by month. An audio-only file has no
  picture to show, so the gallery leaves it out; it is still in the list and grid.

### Preview

Double-click a row, or preview from the details drawer, to open the quick preview:
images, audio, video, and text-like files — source code, configuration, CSV, logs —
render in the browser. Formats the browser cannot render (PDF and Office documents,
for instance) get a download link instead of a fake preview — but their text is still
indexed, so search finds them.

The **details drawer** expands from the bottom when an entry is selected. Beyond type,
size and modified time it holds:

- **Star** — starred entries appear under "Starred".
- **Tags** — free-text labels of your own.
- **EXIF** — the shooting metadata of a photo.
- **Full-text index** — the text the indexer extracted from this file, plus a
  "re-index" button; when a file is not indexed, this section says why.
- **Share links / upload links** — the links already issued for this file.

The drawer has no inline player: its summary thumbnail is the still frame, and the
quick preview plays videos and audio. Uploads, moves, versions and link creation are
in [Files and sharing](/guide/use/files-and-sharing).

## Starred

`/starred/` lists every file and folder you starred. Stars are per-user and do not
affect anybody else.

## Shares

`/shares/` lists the links you created, in two tabs: **share links** (others
download) and **upload links** (others upload). Change a password, an expiry or a
description, or delete a link. See
[Files and sharing](/guide/use/files-and-sharing#manage-existing-links).

## Trash

`/trash/` has two tabs:

- **Files** — deleted files, searchable, restorable, and emptiable with
  **empty trash**.
- **Libraries** — whole libraries, restorable or deleted for good.

Emptying is final: do not press it on something you might want back. The procedure
is in [Files and sharing](/guide/use/files-and-sharing#versions-and-trash).

## Activity

`/activities/` is what happened on the server — created, modified, renamed, moved,
deleted — newest first. An entry links back to its library and directory.

## Search

`/search/` covers two scopes:

- By default it searches **names and content** — the content comes from the text the
  indexer extracted, so PDFs and Office documents are findable too, and a result
  shows the matching snippet. With `[ocr] enabled`, text recognised in images and in
  PDF pages without a text layer is part of that content as well; see
  [Text in images and PDFs](/guide/use/ocr).
- Tick **filename only** to match names alone.

Results keep loading as you scroll. When nothing is found, check that indexing is on:
see [Maintenance](/guide/operate/maintenance).

## Your settings

Pages under `/settings/`:

| Page | Use |
|---|---|
| Overview | account information |
| Profile | display name, avatar, language |
| Security | two-factor authentication, password |
| Sessions & Credentials | devices that signed in, sync tokens, revocation |
| API keys | keys for third-party tools, including WebDAV keys |
| Invitations (admins) | generate registration invitation codes |

Two-factor authentication and credentials are covered in
[Account security](/guide/use/account-security).

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| `/` | focus the search box |
| `g` then `l` | libraries |
| `g` then `s` | starred |
| `g` then `t` | trash |
| `g` then `a` | activity |
| `g` then `p` | your settings |

## Next steps

- [Files and sharing](/guide/use/files-and-sharing) — upload, organize, version and
  share.
- [Connecting clients](/guide/use/clients) — the official Seafile clients and WebDAV.
- [Users and sharing](/guide/admin/users-and-sharing) — accounts, quotas and link
  policy.
- [Maintenance](/guide/operate/maintenance) — where to look when search or thumbnails
  do not work.
