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
  "nickname": "User",
  "id": 2,
  "usage": 22020651,
  "total": 30000000,
  "avatar_url": "https://files.example.com/avatar/render_primary/user@example.com/80/",
  "contact_email": "user@example.com",
  "space_usage": "73.40217%"
}
```

`total` is the storage quota in bytes; `-1` means unlimited. `nickname` is always
present and always equals `name`: the display name when one is set, otherwise the
account's name, otherwise the email's local part. Requires `account.read` for an
API key.

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


## Get an avatar {#get-api2-avatars-user-email-resized-size}

`GET /api2/avatars/user/{email}/resized/{size}/`

Returns the URL of a user's avatar. The URL is absolute and points at the
image-binary route; `size` snaps to one of 32, 48, 64, 72, 80, 96, 128 or 256
(larger requests use 256, above 1024 is a `400`).

**Response `200`**

```json
{ "url": "https://files.example.com/avatar/render_primary/user@example.com/256/", "is_default": false, "mtime": 1760000000 }
```

The endpoint never reveals whether an email exists: an unknown user or a user
without an avatar returns the default-avatar URL with `is_default: true` and
`mtime: 0`. Requires `account.read`.

## Get an avatar image {#get-avatar-image}

`GET /avatar/render_primary/{email}/{size}/`

The image bytes behind `url` above. Served without authentication, because
avatar URLs are embedded in pages and clients; an unknown user or an account
without an avatar gets the default-avatar SVG. Thumbnails are generated and
cached on first access (`Cache-Control: private, max-age=86400`).

The path is seahub's public `avatar_render_primary` view — the one its
`primary_avatar` template tag points `<img>` at. Seahub answers that view with a
redirect to `/media/avatars/…`; this server has no `/media` prefix and serves the
bytes directly instead, so a browser gets the image either way. The
`/avatars/user/{email}/resized/{size}/` spelling released up to 0.1.23 answers a
permanent redirect.

## Upload an avatar {#post-api-v2-1-user-avatar}

`POST /api/v2.1/user-avatar/`

Uploads a new primary avatar for the authenticated account.

**Body** — `multipart/form-data` with an `avatar` file field (also accepted as
`file`). Maximum 1 MiB. The original is stored with its own extension (`.png`,
`.jpg`, `.jpeg` or `.gif`); every thumbnail is rendered as PNG. Requires
`account.write`.

**Response `200`**

```json
{ "avatar_url": "/avatar/render_primary/user@example.com/256/" }
```

## List all users' info {#get-api-v2-1-admin-users}

`GET /api/v2.1/admin/users/?page=&per_page=&is_active=&role=&source=&order_by=&direction=`

Administrator only. Returns accounts in seahub's envelope:

```json
{
  "data": [
    {
      "avatar_url": "https://files.example.com/avatar/render_primary/user@example.com/80/",
      "email": "user@example.com",
      "name": "user",
      "contact_email": "user@example.com",
      "login_id": "",
      "is_staff": false,
      "is_active": true,
      "quota_usage": 22020651,
      "quota_total": 30000000,
      "role": "default",
      "create_time": "2026-10-24T09:00:00+00:00",
      "last_login": "2026-10-24T09:00:00+00:00",
      "last_access_time": "2026-10-24T09:00:00+00:00",
      "social_auth": []
    }
  ],
  "total_count": 1
}
```

`page` defaults to 1 and `per_page` to 25 (at most 500). `is_active` accepts `1`
or `0`. `source` accepts `db` (the default) and `ldapimport` (an always-empty
page: there is no LDAP backend). `role` accepts `default`; any other value
matches no account. `order_by=quota_usage` with `direction=asc|desc` sorts by
usage, and is refused once there are more than 500 accounts.

Item fields follow seahub's `get_user_info()` plus `avatar_url`; differences:
`login_id` is always empty, `contact_email` mirrors `email`, `role` is always
`default`, and `social_auth` is always empty. `quota_total` is bytes (`-1` means
unlimited), `quota_usage` is bytes in use, and the timestamps are ISO 8601.

Requires `admin.user.read`.

**Seafile** — same route and envelope; the differences are the field values
listed above.

## Add user {#post-api-v2-1-admin-users}

`POST /api/v2.1/admin/users/`

Administrator only. Accepts JSON or `application/x-www-form-urlencoded`
(seahub's DRF view accepts both).

| Field | Required | Meaning |
| --- | --- | --- |
| `email` | yes | Login address; must be valid and unique. |
| `password` | yes | Must satisfy `auth.password_min_length` and `require_strong_password`, like every other account-creation path. |
| `is_staff` | no | Default `false`; stored as the account's administrator flag. |
| `is_active` | no | Default `true`. |
| `name` | no | Nickname, at most 64 characters and without `/`. |
| `quota_total` | no | Megabytes; `0` means unlimited, omitted uses the server default. |
| `role` | no | Only `""` or `default`; Nanofile has no custom roles. |

**Response `200`** — the created account in the list-item shape.

`400` for an invalid email, an existing account (`User <email> already exists.`)
or a password below the policy. Requires `admin.user.write`.

Self-registration is a separate web flow (`/accounts/register/`): it enforces
the registration mode, invitation codes, email verification and rate limits.

**Seafile** — same route, body fields and response; `role` is restricted as
described.

## Get a user's info {#get-api-v2-1-admin-users-user-id}

`GET /api/v2.1/admin/users/{user_id}/`

Administrator only. The path segment is the account's **email**: seahub's URL
regex matches an email there even though the reference names the parameter
`{user_id}`.

**Response `200`** — one list item (including `last_login`,
`last_access_time` and `social_auth`). An unknown account is `404`
(`User <email> not found.`). Requires `admin.user.read`.

## Update user info {#put-api-v2-1-admin-users-user-id}

`PUT /api/v2.1/admin/users/{user_id}/`

Administrator only; the segment is the email. A **partial** update: omitted
fields keep their current values.

| Field | Meaning |
| --- | --- |
| `is_staff` | Administrator flag. |
| `is_active` | Deactivating revokes the account's sessions, sync tokens, API keys and outstanding capability URLs immediately. |
| `quota_total` | Megabytes; `0` means unlimited. |
| `name` | Nickname; an empty string clears it. |
| `password` | New password, subject to the configured policy. |
| `role` | Only `""` or `default`. |

**Response `200`** — the updated list item plus
`"update_status_tip": ""` (this server sends no notification email). Requires
`admin.user.write`.

`login_id`, `contact_email`, `institution` and the Pro rate-limit fields have no
storage here and are ignored; there is no `keep_sharing` switch (deactivation
keeps shares, as it does in seahub's default mode).

## Delete user {#delete-api-v2-1-admin-users-user-id}

`DELETE /api/v2.1/admin/users/{user_id}/`

Administrator only; the segment is the email. Deletes the account and its data.
**Response `200`** — `{"success": true}`. Requires `admin.user.write`.

An account that owns a shared or public library cannot be deleted until that
library is made private again (`400`); this server refuses rather than silently
revoking every recipient.
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
like seahub; 4 (per-library salt) is also supported. Clients take the advertised
value as the `enc_version` of a library they create, so with the default 2 they
create v2 libraries; set it to 4 to make new libraries use the per-library salt.
The same value drives server-side key generation when a client creates an
encrypted library with only `passwd`.
