# Authentication

Credentials, login and the endpoints a client uses to obtain and discard them.
See the [overview](/reference/api/#authentication) for the four credential types
and which surfaces accept them.

Source: `server/src/handler/login.rs`, `server/src/handler/client_login.rs`,
`server/src/handler/sso.rs`, `server/src/handler/device_wipe.rs`;
tests: `server/tests/auth_test.rs`, `server/tests/client_compat_test.rs`.

## Obtain an account token {#post-api2-auth-token}

`POST /api2/auth-token/`

Exchanges an account's username and password for an account token. The token is
accepted as `Authorization: Bearer <token>` (or the legacy
`Authorization: Token <token>`) on every API surface.

**Body** — URL-encoded form by default; JSON and `multipart/form-data` are also
accepted, chosen by `Content-Type`.

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `username` | string | yes | The account's email address. |
| `password` | string | yes | The password. |
| `platform`, `device_id`, `device_name`, `client_version` | string | no | Device metadata recorded with the token and shown under sessions. |

**Two-factor authentication** — the OTP is not part of the body; it travels in
headers, matching seahub:

| Header | Meaning |
| --- | --- |
| `X-SEAFILE-OTP` | The 6-digit TOTP code. |
| `X-SEAFILE-S2FA` | A session token returned by a previous successful login, when the account has 2FA enabled. |
| `X-SEAFILE-2FA-TRUST-DEVICE` | `1` to trust this device for 90 days. |

**Response `200`**

```json
{ "token": "…" }
```

When the request asked to trust the device
(`X-SEAFILE-2FA-TRUST-DEVICE: 1`) and the OTP was valid, the response also
carries an `X-Seafile-S2FA` header. Store that value and send it as
`X-SEAFILE-S2FA` on the next login from the same device to skip the code.

**Errors** — `400` with DRF's serializer shape:

```json
{ "non_field_errors": ["Unable to login with provided credentials."] }
```

Other messages cover a disabled account, an unverified email address, too many
attempts, a missing OTP (`X-Seafile-OTP: required` header is set) and an invalid
OTP. Repeated failures lock the client address out for
`auth.lockout_duration_secs`.

**Seafile** — same route and same token contract. Nanofile additionally accepts
JSON and multipart bodies (the Android client posts multipart).

## Authenticated ping {#get-api2-auth-ping}

`GET /api2/auth/ping/`

Checks that the credential works. Any valid credential is accepted; the body is
the JSON string `"pong"`.

## Log out the current token {#post-api2-logout-device}

`POST /api2/logout-device/`

Invalidates the account token used for the request. The credential must be
presented in the `Authorization` header; a session cookie alone cannot name the
token to revoke.

**Response `200`** — `{"success": true}`.

## One-time browser login token {#post-api2-client-login}

`POST /api2/client-login/`

Mints a token for the desktop client's "open in browser" action. The client then
opens `{site_url}/client-login/?token=…`, which signs the browser in. The token
is bound to the requesting address and expires after 30 seconds.

**Response `200`** — `{"token": "…"}`. Requires an account credential (session or
account token); API keys are rejected.

## Create a client SSO link {#post-api2-client-sso-link}

`POST /api2/client-sso-link/`

Starts the browser-based single sign-on flow used by the official clients. This
endpoint is anonymous by protocol: the mobile clients post no body and no
credential, the desktop client passes its device details as query parameters.

| Query parameter | Meaning |
| --- | --- |
| `shib_platform` | Client platform, at most 128 characters. |
| `shib_device_id` | Device id. |
| `shib_device_name` | Display name shown on the confirmation page. |
| `shib_client_version` | Client version. |

**Response `200`**

```json
{ "link": "https://files.example.com/client-sso/<token>/" }
```

The link always uses the configured `server.site_url`, never the request's
`Host` header. Creation is rate limited per client address
(`auth.sso_link_max_per_hour`).

## Poll a client SSO link {#get-api2-client-sso-link-token}

`GET /api2/client-sso-link/{token}/`

Polls the state of an SSO link. Public, like the creation call.

**Response `200`** — one of:

```json
{ "status": "waiting" }
{ "status": "success", "username": "user@example.com", "apiToken": "…" }
{ "status": "error" }
```

Unknown tokens answer `404`. `success` is returned once; the client stores
`apiToken` as its account token.

## Report a wiped device {#post-api2-device-wiped}

`POST /api2/device-wiped/`

A client that was wiped remotely reports itself so the server can drop the
credentials it holds. Public by protocol, authenticated by the token being
revoked: either `Authorization: Bearer <token>`/`Token <token>`, or the token in
a URL-encoded `token` form field (what the desktop client sends).

**Response `200`** — `{"success": true}`.

The token must be a full, unexpired account token. A pending 2FA token is
refused. The call revokes the token's device sessions and every outstanding
capability URL (upload/download links) for that account.

**Seafile** — same route; both the desktop's form field and Android's
authorization header shapes are accepted.
