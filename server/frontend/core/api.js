// api — authenticated fetch helper.
import { getCookie } from "./utils.js";

export async function apiFetch(url, options) {
  options = options || {};
  var headers = options.headers || {};
  if (!headers["X-CSRFToken"]) {
    headers["X-CSRFToken"] = getCookie("sfcsrftoken");
  }
  if (
    options.body &&
    !(options.body instanceof FormData) &&
    typeof options.body === "string" &&
    !headers["Content-Type"]
  ) {
    headers["Content-Type"] = "application/json;charset=utf-8";
  }
  options.headers = headers;

  var res = await fetch(url, options);
  if (!res.ok) {
    var text = await res.text().catch(function () { return res.statusText; });
    throw new Error(text || res.statusText);
  }
  return res;
}
