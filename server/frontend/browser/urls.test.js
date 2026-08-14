import { test } from "node:test";
import assert from "node:assert/strict";
import { buildListUrl, buildEntryApiPath } from "./urls.js";

test("buildListUrl builds refresh url for all views", () => {
  assert.equal(
    buildListUrl({
      pathname: "/libraries/1/files/",
      view: "all",
      page: null,
      sort: { sort: "name", sort_order: "asc" },
      tag: "",
    }),
    "/libraries/1/files/?partial=1&view=all&sort=name&sort_order=asc",
  );
});

test("buildListUrl adds page and forces mtime-desc for gallery", () => {
  assert.equal(
    buildListUrl({
      pathname: "/libraries/1/files/",
      view: "gallery",
      page: 2,
      sort: { sort: "name", sort_order: "asc" },
      tag: "",
    }),
    "/libraries/1/files/?partial=1&view=gallery&page=2&sort=mtime&sort_order=desc",
  );
});

test("buildListUrl uses & separator when pathname has a query", () => {
  assert.equal(
    buildListUrl({
      pathname: "/libraries/1/files/?x=1",
      view: "list",
      page: null,
      sort: { sort: "size", sort_order: "desc" },
      tag: "work",
    }),
    "/libraries/1/files/?x=1&partial=1&view=list&sort=size&sort_order=desc&tag=work",
  );
});

test("buildListUrl encodes tag and omits sort when absent", () => {
  assert.equal(
    buildListUrl({
      pathname: "/libraries/1/files/",
      view: "list",
      page: null,
      sort: null,
      tag: "a b",
    }),
    "/libraries/1/files/?partial=1&view=list&tag=a%20b",
  );
});

test("buildEntryApiPath builds dir vs file paths", () => {
  assert.equal(buildEntryApiPath("r1", "/a b/c", "dir"), "/api2/repos/r1/dir/?p=%2Fa%20b%2Fc");
  assert.equal(buildEntryApiPath("r1", "/a b/c", "file"), "/api2/repos/r1/file/?p=%2Fa%20b%2Fc");
});
