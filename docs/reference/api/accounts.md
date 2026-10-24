# Accounts and administration

The authenticated account's own profile, avatars, the small administrative user
surface, and the public server-information endpoints.

Source: `server/src/handler/account.rs`, `server/src/handler/avatar.rs`,
`server/src/handler/user_avatar.rs`, `server/src/handler/users.rs`,
`server/src/handler/server_info.rs`, `server/src/handler/notifications.rs`,
`server/src/service/user/account.rs`.

## Get account info {#get-api2-account-info}

`GET /api2/account/info/`

Returns the authenticated account and its quota usage.

**Response `200`**

```json
{
  "email": "user@example.com",
  "name": "User",
  "nickname": "Display name",
  "id": 2,
  "usage": 22020651,
  "total": 30000000,
  "avatar_url": "https://files.example.com/avatars/user/user@example.com/resized/256/",
  "contact_email": "user@example.com",
  "space_usage": "73.40217%"
}
```

`total` is the storage quota in bytes; `-1` means unlimited. `nickname` is omitted
when the account has no display name. Requires `account.read` for an API key.

**Seafile** — same route. The response omits seahub's `login_id`, `is_staff`,
`email_notification_interval`, `institution` and `department` fields; none of the
official clients read them.

## Update account info {#put-api2-account-info}

`PUT /api2/account/info/`

Changes the display name. The official clients send `{"name": "…"}` when the
user edits their nickname.

**Body** — JSON: `{"name": "New name"}` (required by the extractor).

**Response `200`** — the same shape as `GET /api2/account/info/`.

Requires `account.write`; keys carrying it may call this.

## Create an account {#post-api2-account}

`POST /api2/account/`

Administrator only. Creates an account from `email` and `password` form fields;
the password policy applies. The same handler is mounted at
`POST /api2/accounts/` for clients that used seahub's second spelling.

**Body** — URL-encoded form:

| Field | Required | Meaning |
| --- | --- | --- |
| `email` | yes | Login address; must be unique. |
| `password` | yes | Must satisfy `auth.password_min_length` and the strength policy. |

**Response `200`** — empty body. `403` for a non-administrator.

### Create an account (second spelling) {#post-api2-accounts}

`POST /api2/accounts/`

The same handler as `POST /api2/account/`, for clients that used seahub's
other spelling. Same form fields, response and permission.

Self-registration is a web flow (`/accounts/register/`), not this endpoint: it
enforces the registration mode, invitation codes, email verification and rate
limits.

## Get an avatar {#get-api2-avatars-user-email-resized-size}

`GET /api2/avatars/user/{email}/resized/{size}/`

Returns the URL of a user's avatar. The URL is absolute and points at the
image-binary route; `size` snaps to one of 32, 48, 64, 72, 80, 96, 128 or 256
(larger requests use 256, above 1024 is a `400`).

**Response `200`**

```json
{ "url": "https://files.example.com/avatars/user/user@example.com/resized/256/", "is_default": false, "mtime": 1760000000 }
```

The endpoint never reveals whether an email exists: an unknown user or a user
without an avatar returns the default-avatar URL with `is_default: true` and
`mtime: 0`. Requires `account.read`.

The image itself is served without authentication at
`GET /avatars/user/{email}/resized/{size}/` (SVG default when absent), because
avatar URLs are embedded in pages and clients.

## Upload an avatar {#post-api-v2-1-user-avatar}

`POST /api/v2.1/user-avatar/`

Uploads a new primary avatar for the authenticated account.

**Body** — `multipart/form-data` with an `avatar` file field (also accepted as
`file`). Maximum 1 MiB; the image is re-encoded to PNG. Requires
`account.write`.

**Response `200`**

```json
{ "avatar_url": "/avatars/user/user@example.com/resized/256/" }
```

## List users {#get-api2-admin-users}

`GET /api2/admin/users/`

Administrator only. Returns every account.

**Response `200`**

```json
[
  {
    "id": 2,
    "email": "user@example.com",
    "is_active": true,
    "is_admin": false,
    "storage_quota": 30000000,
    "usage": 22020651,
    "created_at": 1760000000,
    "last_login_at": 1760000000
  }
]
```

Requires `admin.user.read`. This is Nanofile's own administration surface; it is
not seahub's `/api/v2.1/admin/users/` API, whose field names and options differ.

## Create a user {#post-api2-admin-users}

`POST /api2/admin/users/`

Administrator only.

**Body** (JSON)

| Field | Required | Meaning |
| --- | --- | --- |
| `email` | yes | Login address. |
| `password` | yes | Must satisfy the password policy. |
| `is_admin` | no | Default `false`. |
| `is_active` | no | Default `true`. |
| `storage_quota` | no | Bytes; `null` uses the server default. |

**Response `200`** — the created user in the list shape. Requires
`admin.user.write`.

## Update a user {#put-api2-admin-users-user-id}

`PUT /api2/admin/users/{user_id}/`

Administrator only. Replaces the account's administration flags.

**Body** (JSON) — `is_admin`, `is_active` and `storage_quota` are all read; send
the full state you want. `user_id` is the numeric `id` from the list.

**Response `200`** — the updated user. Requires `admin.user.write`.

## Delete a user {#delete-api2-admin-users-user-id}

`DELETE /api2/admin/users/{user_id}/`

Administrator only. Deletes the account and its data. **Response `200`** —
`{"success": true}`. Requires `admin.user.write`.

## Unseen messages {#get-api2-unseen-messages}

`GET /api2/unseen_messages/`

Seahub's notification counter. Nanofile has no notification inbox, so the count
is always `0`.

**Response `200`** — `{"count": 0}`. Requires `notification.read`.

## Ping {#get-api2-ping}

`GET /api2/ping/`

Public reachability probe.

**Response `200`** — the JSON string `"pong"`.

## Server information {#get-api2-server-info}

`GET /api2/server-info/`

Public. Reports the Seafile version this server advertises and the client-facing
feature flags.

**Response `200`**

```json
{
  "version": "8.0.0",
  "encrypted_library_version": 2,
  "features": ["seafile-basic", "seafile-pro", "file-search"]
}
```

`version` is `server.version`; clients parse it and gate features on it.
`desktop-custom-brand` and `desktop-custom-logo` appear only when configured, and
`encrypted_library_pwd_hash_algo`/`encrypted_library_pwd_hash_params` only when
the server advertises a client-side password-hash algorithm. `features` always
contains `seafile-basic` and `seafile-pro`; `file-search` and
`client-sso-via-local-browser` follow the configuration, and
`share-link-disabled` is added when share downloads are turned off.

**Seafile** — same route and shape. `encrypted_library_version` defaults to 2,
like seahub; 4 (per-library salt) is also supported and is what clients use for
new encrypted libraries.
