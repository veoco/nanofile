# API keys and credentials

Nanofile's own credential model: scoped API keys for scripts and third-party
tools, and the inventory of everything long-lived an account holds. None of this
is part of Seafile's API; the official clients do not use it.

Key management is **session-only** (`POST /api2/auth-token/` tokens included):
an API key can never mint or read another key. A key that could do so would be
able to widen its own access.

Source: `server/src/handler/api_key.rs`, `server/src/handler/credential.rs`,
`server/src/handler/devices.rs`, `server/src/service/api_key.rs`,
`server/src/domain/capability.rs`.

## Capabilities

A key is a set of independently grantable capabilities plus a library scope. An
endpoint's page names the capability it requires. Capabilities are grouped by
domain:

| Domain | Capability ids |
| --- | --- |
| account | `account.read`, `account.write` |
| device | `device.read`, `device.write` |
| library | `library.read`, `library.create`, `library.update`, `library.delete`, `library.password` |
| file | `file.read`, `file.write`, `file.delete` |
| trash | `trash.read`, `trash.write` |
| history | `history.read`, `history.write` |
| sharing | `share_link.read`, `share_link.write`, `upload_link.read`, `upload_link.write` |
| metadata | `metadata.read`, `metadata.write`, `tag.read`, `tag.write` |
| discovery | `star.read`, `star.write`, `activity.read`, `search.read`, `search.write`, `search.reindex` |
| sync | `sync.read`, `sync.write`, `sync.token` |
| notification | `notification.read` |
| webdav | `webdav.read`, `webdav.write` |
| admin | `admin.user.read`, `admin.user.write` |
| key | `key.read`, `key.write` (reserved: never granted, because key management is session-only) |

A write capability implies the matching read capability; `sync.token` also
implies `sync.write` and `sync.read`. Unknown ids are rejected. The live list is
[`GET /api2/api-keys/catalog/`](#get-api2-api-keys-catalog), which also returns
each capability's `enforced_by` routes.

## Presets

The catalog exposes presets; they expand to ordinary capability sets and can be
edited before saving.

| Preset | Purpose |
| --- | --- |
| `sync_client_rw`, `sync_client_ro` | A third-party sync client |
| `webdav_rw`, `webdav_ro` | A WebDAV client |
| `ci_upload` | Upload into one library without deleting |
| `readonly_all` | Read-only across the user surface |
| `full` | Everything except the reserved `key.*` |

## Get the capability catalog {#get-api2-api-keys-catalog}

`GET /api2/api-keys/catalog/`

Returns the capabilities, presets and lifetime options, so a client can render
the same picker as the settings page.

**Response `200`**

```json
{
  "capabilities": [
    { "id": "file.read", "domain": "file", "write": false, "enforced_by": ["GET /api2/repos/{repo_id}/file/"] }
  ],
  "presets": [{ "id": "readonly_all", "capabilities": ["account.read"] }],
  "ttl_presets_days": [7, 30, 90, 180, 365],
  "max_ttl_days": 0
}
```

The set of capabilities returned depends on whether the caller is an
administrator: the `admin.*` capabilities are only offered to admins.
`max_ttl_days` is `0` when the server sets no upper bound.

## List API keys {#get-api2-api-keys}

`GET /api2/api-keys/`

**Response `200`**

```json
{
  "keys": [
    {
      "id": 1,
      "name": "backup script",
      "key_prefix": "a1b2c3d4",
      "capabilities": ["library.read", "file.read"],
      "all_repos": false,
      "repos": [{ "repo_id": "…", "permission": "r" }],
      "created_at": 1760000000,
      "expires_at": 1762592000,
      "last_used_at": null
    }
  ]
}
```

Secrets are never returned here; only the first 8 characters are kept for
display.

## Create an API key {#post-api2-api-keys}

`POST /api2/api-keys/`

**Body** (JSON)

| Field | Type | Required | Meaning |
| --- | --- | --- | --- |
| `name` | string | no | Display name, at most 255 characters. |
| `capabilities` | string[] | yes | Capability ids. |
| `all_repos` | bool | no | Apply to every library the account can see. Default `false`. |
| `repo_permissions` | array | no | `[{"repo_id": "…", "permission": "r" \| "rw"}]`, used when `all_repos` is false. |
| `expires_in_days` | integer | one of | Lifetime in days. |
| `expires_at` | integer | one of | Absolute expiry, Unix seconds. |
| `never` | bool | one of | Never expire. |

Exactly one lifetime field is required. A key may not be created with a lifetime
beyond `auth.api_key_max_ttl_days`.

**Response `200`** — the key view with an extra `key` field. That plaintext value
is returned **once**; only a hash is stored.

Saving sends a best-effort notification email to the owner.

## Get one API key {#get-api2-api-keys-id}

`GET /api2/api-keys/{id}/`

Returns one key's view (same shape as a list item). Unknown ids and other users'
keys answer `404`.

## Update an API key {#put-api2-api-keys-id}

`PUT /api2/api-keys/{id}/`

Accepts any subset of `name`, `capabilities`, `all_repos`,
`repo_permissions`, `expires_in_days`, `expires_at`, `never`. Omitted fields are
left alone; omitting every lifetime field keeps the current expiry. A lifetime
field, when present, must be the only one.

**Response `200`** — the updated view.

## Delete an API key {#delete-api2-api-keys-id}

`DELETE /api2/api-keys/{id}/`

Revokes the key immediately.

**Response `200`** — `{"success": true}`.

## List credentials {#get-api2-credentials}

`GET /api2/credentials/`

Every long-lived credential the account holds: client sessions, browser
sessions, sync tokens, trusted devices, and the number of API keys (whose
contents are only readable through the session-only key endpoints).

**Response `200`**

```json
{
  "client_sessions": [
    { "platform": "linux", "platform_display": "Linux", "device_id": "…", "device_name": "laptop", "client_version": "9.0.0", "created_at": 1760000000, "is_desktop_client": true }
  ],
  "browser_sessions": [{ "id": 1, "source": "web", "client": "…", "created_at": 1760000000 }],
  "sync_tokens": [{ "id": 1, "repo_id": "…", "repo_name": "Docs", "device_name": "laptop", "peer_ip": "192.0.2.10", "client_version": "9.0.0", "created_at": 1760000000, "last_sync_at": 1760000000, "expires_at": null }],
  "device_trusts": [{ "id": 1, "device_id": "…", "device_name": "phone", "created_at": 1760000000, "expires_at": 1762592000 }],
  "api_key_count": 2
}
```

Token values are never included. Requires `device.read`.

## Revoke one credential {#delete-api2-credentials-kind-id}

`DELETE /api2/credentials/{kind}/{id}/`

`kind` is `browser_session`, `sync_token` or `device_trust`; `id` is the row id
from the inventory. Client devices are revoked as a whole through
[`DELETE /api2/devices/`](#delete-api2-devices), because one device owns several
credentials. Requires `device.write`.

**Response `200`** — `{"revoked": "sync_token", "id": 1}`.

## List devices {#get-api2-devices}

`GET /api2/devices/`

The Seafile-compatible device list. Nanofile returns one array containing every
credential row of the account, each tagged with a `kind` (`client`, `browser`,
`sync_token`, `device_trust`), so a client that only reads the Seafile fields
still finds the client devices.

**Response `200`**

```json
[
  {
    "kind": "client",
    "key": "linux:…",
    "platform": "linux",
    "device_id": "…",
    "device_name": "laptop",
    "client_version": "9.0.0",
    "last_accessed": "2026-10-24 09:00",
    "is_desktop_client": true
  }
]
```

Requires `device.read`.

## Unlink a device {#delete-api2-devices}

`DELETE /api2/devices/`

Revokes a credential. Two addressings are accepted:

- `platform` + `device_id` (form fields) unlink everything that device owns —
  the shape the official clients post.
- `kind` + `id` revoke one inventory row.

**Response `200`** — `{"success": true}`. Requires `device.write`.

**Seafile** — `GET`/`DELETE /api2/devices/` exist upstream with the same
addressings; Nanofile's response adds the extra credential kinds and their
`kind` tags. `api-keys` and `credentials` are Nanofile-only.
