// urls — pure URL/path builders for the file browser (no DOM, no globals).

// Build the partial-refresh list URL shared by refreshFileList (view="all",
// no page) and loadMoreEntries (actual view + next page). Gallery always uses
// mtime-desc sort so groups stay reverse-chronological.
export function buildListUrl(params) {
  var pathname = params.pathname;
  var view = params.view;
  var page = params.page;
  var sort = params.sort;
  var tag = params.tag;

  var sep = pathname.indexOf("?") !== -1 ? "&" : "?";
  var url = pathname + sep + "partial=1&view=" + view;
  if (page) url += "&page=" + page;
  if (view === "gallery") {
    url += "&sort=mtime&sort_order=desc";
  } else if (sort) {
    url += "&sort=" + sort.sort + "&sort_order=" + sort.sort_order;
  }
  if (tag) url += "&tag=" + encodeURIComponent(tag);
  return url;
}

// Build the /api2/repos/{id}/{dir|file}/?p=... mutation path shared by the
// delete and rename flows.
export function buildEntryApiPath(repoId, path, entryType) {
  return entryType === "dir"
    ? "/api2/repos/" + repoId + "/dir/?p=" + encodeURIComponent(path)
    : "/api2/repos/" + repoId + "/file/?p=" + encodeURIComponent(path);
}
