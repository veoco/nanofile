# Web interface

Once you are signed in, the file manager is what you see first. This page follows
the interface's own shape: the left rail, libraries, the file browser, then
starred items, shares, trash, activity, search and your settings. Administrators
also reach the admin console, covered in
[Configuration](/guide/configuration) and
[Users and sharing](/guide/users-and-sharing).

## The frame

- **Left rail** — the library tree on top (with its own filter box), and a
  "Browse" block below it: starred, shares, trash, activity. At the bottom, your
  storage meter and the version.
- **Top-bar search box** — Enter opens the search page, see [Search](#search).
- **Details drawer** — rises from the bottom when you select an entry, see
  [The file browser](#the-file-browser).

| Shortcut | Action |
|---|---|
| `/` | focus the search box |
| `g` then `l` | libraries |
| `g` then `s` | starred |
| `g` then `t` | trash |
| `g` then `a` | activity |
| `g` then `p` | your settings |

## Libraries

`/libraries/` lists the libraries you can reach, sortable by name, modified time
or size, and filterable.

- **New library** sits in the page header. The web interface creates a **regular**
  library; an **encrypted library is created by a client** (it carries a lock
  badge in the list), and its password is entered in the client — the server
  cannot supply it.
- The **⋯** menu on a row opens **edit**: change the name and description, set
  **history retention** (how many versions to keep, how many days to keep them),
  and see that library's WebDAV address.
- Deleting a library moves it to the "Libraries" tab of the trash; it does not
  vanish on the spot.

## The file browser

`/libraries/<library id>/files/` is the file manager.

### Three views

- **List** — name, size, modified time.
- **Grid** — tiles with thumbnails.
- **Gallery** — images and videos grouped by month (an audio-only file has no
  picture to show, so the gallery leaves it out; it is still in the list and
  grid).

### Uploading

The toolbar has **new folder**, **upload folder** and **upload file**; you can
also **drag files or a folder onto the list** and drop them into the current
directory. A progress panel opens in the bottom-right corner and can be
minimised while the upload continues.

### Actions on one entry

The **⋯** menu at the end of a row (or the details drawer):

- **Share link** / **upload link** — create one and change its password, see
  [Users and sharing](/guide/users-and-sharing).
- **Download** — a file streams directly; a folder is zipped.
- **History** — the file's version history, where earlier versions are reachable.
- **Rename** and **Delete**.

With several entries selected, the drawer offers bulk **download, move, copy,
re-index and delete**.

### The details drawer

It expands from the bottom when an entry is selected. Beyond type, size and
modified time it holds:

- **Star** — starred entries appear under "Starred".
- **Tags** — free-text labels of your own.
- **EXIF** — the shooting metadata of a photo.
- **Full-text index** — the text the indexer extracted from this file, plus a
  "re-index" button; when a file is not indexed, this section says why.
- **Share links / upload links** — the links already issued for this file.

### Preview

Double-clicking a row (or previewing from the drawer) opens a quick preview:
images, audio and video, and text-like files — source code, configuration, CSV,
logs — render in the browser. Formats the browser cannot render (PDF and Office
documents, for instance) get a download link instead of a fake preview — but
their text is still indexed, so search finds them.

## Starred

`/starred/` lists every file and folder you starred. Stars are per-user and do
not affect anybody else.

## Shares

`/shares/` has two tabs: **share links** (others download) and **upload links**
(others upload). Each one can have its password changed (keep the current one,
set a new one, or remove it), its expiry changed (1/7/30 days, or cleared) and
its description edited, and can be deleted.

## Trash

`/trash/` has two tabs as well:

- **Files** — deleted files, searchable, **restorable**, and emptiable with
  **empty trash**.
- **Libraries** — whole libraries, restorable or **deleted for good**.

Emptying is final: do not press it on something you might want back.

## Activity

`/activities/` is what happened on the server — created, modified, renamed,
moved, deleted — newest first. An entry links back to its library and directory.

## Search

`/search/` covers two scopes:

- By default it searches **names and content** — the content comes from the text
  the indexer extracted, so PDFs and Office documents are findable too, and a
  result shows the matching snippet.
- Tick **filename only** to match names alone.

Results keep loading as you scroll. When nothing is found, check that indexing is
on: see [Maintenance and troubleshooting](/guide/maintenance).

## Your settings

Pages under `/settings/`:

| Page | Use |
|---|---|
| Overview | account information |
| Profile | display name, avatar, language |
| Security | two-factor authentication, password |
| Sessions & credentials | devices that signed in, sync tokens, revocation |
| API keys | keys for third-party tools, including WebDAV keys |
| Invitations (admins) | generate registration invitation codes |

Two-factor authentication and credentials are covered in
[Account security](/guide/account-security).

## Next steps

- [Connecting clients](/guide/clients) — the official Seafile clients and WebDAV.
- [Users and sharing](/guide/users-and-sharing) — accounts, quotas, link policy.
- [Maintenance and troubleshooting](/guide/maintenance) — where to look when
  search or thumbnails do not work.
