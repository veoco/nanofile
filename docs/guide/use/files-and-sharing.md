---
description: Upload, download, organize, version and share files from the web interface.
---
# Files and sharing

This page covers the file manager itself: getting files in and out, organizing them,
restoring them, and creating share or upload links. For the surrounding interface,
see [Web interface](/guide/use/web-interface).

**Who this is for:** anybody signed in to the web interface. **Before you start:**
you need read/write permission on the library for uploads, moves, deletes and link
creation; a read-only library still allows downloads.

## Upload and download

### Upload

- Use **upload file**, **upload folder** or **new folder** in the toolbar.
- Or drag files or a folder from your desktop and drop them onto the file list; they
  go into the directory you are viewing.
- A progress panel opens in the bottom-right corner. You can minimise it while the
  upload continues.

### Download

- A file downloads directly.
- A folder downloads as a zip archive.
- To download several entries, select them and use **download** in the details
  drawer.

## Views and preview

The file browser has three views:

- **List** — name, size and modified time.
- **Grid** — tiles with thumbnails.
- **Gallery** — images and videos grouped by month. Audio-only files have no
  thumbnail, so the gallery leaves them out; they remain in the list and grid.

Double-click a row, or use **preview** in the details drawer, to open the quick
preview. Images, audio, video and text-like files (source code, configuration, CSV,
logs) render in the browser. PDFs and Office documents get a download link instead
of a fake preview, but their text is still indexed for search.

## Organize files

- The row's **⋯** menu has **rename** and **delete**.
- Select several entries to bulk **download**, **move**, **copy**, **re-index** or
  **delete**.
- The details drawer shows type, size, modified time, stars, tags, EXIF and the
  extracted full-text index. It also carries the **re-index** button for one file.
- Deleting a file moves it to the trash; it does not free the block immediately. See
  [Maintenance](/guide/operate/maintenance) for garbage collection.

## Versions and trash

- **History** on a file opens its version history; earlier versions are reachable
  from there.
- **Trash → Files** holds deleted files. They are searchable and restorable, and the
  tab has **empty trash**.
- **Trash → Libraries** holds whole deleted libraries. They can be restored or
  deleted for good.

Emptying the trash is final. Restore first, or download a copy before you empty it.

## Share links

A share link lets somebody without an account download a file or a folder.

1. Select the file or folder and choose **share link** in the row menu or the
   details drawer.
2. Set a password, an expiry time (or none) and, for a share link, a view limit.
3. Copy the link and send it.

The link opens a public page with the file or folder. A password-protected link asks
for the password first. An expired or used-up link answers as gone.

## Upload links

An upload link lets somebody without an account upload into the current library and
directory. Create it the same way as a share link, then send the link. The recipient
does not need a Nanofile account.

- Upload links support a password and an expiry time.
- They cannot target an encrypted library. The server does not have the library
  password, so it cannot write the uploaded content.
- An upload link created before an administrator turned upload sharing off is refused
  on use.

## Manage existing links

**Shares** in the left rail lists the links you created, in two tabs: **share links**
and **upload links**. For each link you can:

- keep, replace or remove the password;
- change or clear the expiry (1, 7 or 30 days, or no expiry);
- edit the description;
- delete the link.

An administrator can see every user's links under **System Settings → Share
Management** and delete the dead ones. The switches that turn download sharing or
upload sharing off are on **System Settings → Security → Sharing**; see
[Users and sharing](/guide/admin/users-and-sharing#sharing-policy).

## Verify

- Open a new share link in a private window; confirm the password prompt and the
  download.
- Open an upload link in a private window and upload a small file; confirm it appears
  in the target directory.
- Delete a test file, restore it from **Trash → Files**, and download it again.

## Related

- [Web interface](/guide/use/web-interface) — the surrounding interface and search.
- [Users and sharing](/guide/admin/users-and-sharing) — sharing policy and quotas.
- [Account security](/guide/use/account-security) — sessions, two-factor and revocation.
- [Maintenance](/guide/operate/maintenance) — garbage collection and index rebuilds.
