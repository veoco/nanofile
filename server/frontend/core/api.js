// api — authenticated fetch helper.
import { getCookie } from "./utils.js";

// The session cookie's CSRF token, or a redirect to the login page. A missing
// token means the session expired; the page modules each copied this check.
export function requireLogin() {
  var token = getCookie("sfcsrftoken");
  if (!token) {
    window.location.href = "/accounts/login/";
    return "";
  }
  return token;
}

// A state-changing request: CSRF, JSON encoding and the login redirect in one
// place. Returns the response, or `null` when the caller was redirected to the
// login page (so a `.then` never runs a success branch after navigation).
export async function mutate(url, options) {
  var opts = options || {};
  var token = requireLogin();
  if (!token) return null;
  var headers = Object.assign({}, opts.headers || {});
  if (!headers["X-CSRFToken"]) headers["X-CSRFToken"] = token;
  var body = opts.body;
  if (
    body !== undefined &&
    body !== null &&
    typeof body === "object" &&
    !(body instanceof FormData)
  ) {
    body = JSON.stringify(body);
  }
  return apiFetch(url, {
    method: opts.method || "POST",
    headers: headers,
    body: body,
  });
}

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
