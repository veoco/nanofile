# 目录

列目录，以及创建、重命名、移动、删除或把目录提升为资料库。文件内容见 [文件与批量操作](/zh/reference/api/files)，传输见 [上传与下载](/zh/reference/api/transfers)。

目录列表是 Seafile 客户端最敏感的地方：旧版响应带 `oid` 与 `dir_perm` 头，每个条目带调用者的 `permission`。原因见 [Seafile 兼容性](/zh/dev/protocol#列表响应)。

源码：`server/src/handler/dir.rs`、`server/src/service/fs/dir.rs`；测试：`server/tests/client_compat_test.rs`、`server/tests/api_v21_test.rs`。

## 列出目录内容 {#get-api2-repos-repo-id-dir}

`GET /api2/repos/{repo_id}/dir/?p=<path>&t=<f|d>&recursive=<0|1>`

列出一个目录。路径是绝对路径，`/` 是根。

| 查询参数 | 含义 |
| --- | --- |
| `p` | 目录路径，默认 `/`。 |
| `t` | `f` 只看文件，`d` 只看目录。其他值返回 `400`。 |
| `recursive` | `1` 深度优先遍历整棵子树；`_Internal` 被隐藏。 |
| `reloaddir` | 本接口忽略；见下面的写操作。 |

**响应 `200`** —— 目录项的裸 JSON 数组：

```json
[
  { "id": "…", "type": "dir", "name": "photos", "size": 0, "mtime": 1760000000, "permission": "rw", "modifier_email": "" },
  { "id": "…", "type": "file", "name": "notes.txt", "size": 1204, "mtime": 1760000000, "permission": "rw", "modifier_email": "user@example.com" }
]
```

目录在前、文件在后，各自按小写名称排序——这是 seahub 的展示顺序，不是对象存储顺序。递归列表按深度优先，条目额外带 `parent_dir`；文件条目还带 `modifier_name` 与 `modifier_contact_email`。

**响应头** —— `oid` 是所列出目录的 FS 对象 id（40 位十六进制），`dir_perm` 是 `"rw"` 或 `"r"`。尚无提交的资料库返回空列表，`oid: 0000000000000000000000000000000000000000`。

需要读权限与 `file.read`。

**Seafile** —— 路由、顺序与响应头相同。`modifier_email` 出现在每个条目上，而 seahub 只对文件记录；官方客户端在目录条目上会忽略它。

## 创建或重命名目录 {#post-api2-repos-repo-id-dir}

`POST /api2/repos/{repo_id}/dir/`

创建目录；`operation=rename` 时重命名。请求体可以是 JSON、URL 编码表单或 multipart。

| 字段 | 必填 | 含义 |
| --- | --- | --- |
| `p` | 是 | 新目录的绝对路径（也可作为查询参数 `p`）。 |
| `operation` | 否 | `rename` 把 `p` 就地改名为 `newname`。 |
| `newname` | rename 时 | 新基名。 |

**响应** —— 创建返回 `201`，带指向新目录的 `Location` 头与 JSON 字符串 `"success"`。带 `reloaddir=true` 时改为返回父目录的列表（iOS 客户端期望的裸数组），带同样的 `oid`/`dir_perm` 头。重命名返回 `200` 与 `"success"`。

需要写权限与 `file.write`。

## 删除目录 {#delete-api2-repos-repo-id-dir}

`DELETE /api2/repos/{repo_id}/dir/?p=<path>`

删除目录及其全部内容，并记入资料库回收站。带 `reloaddir=true` 时返回父目录列表；否则返回 `{"success": true}`。

需要写权限与 `file.delete`。

## 列出目录内容（v2.1） {#get-api-v2-1-repos-repo-id-dir}

`GET /api/v2.1/repos/{repo_id}/dir/?p=<path>&t=<f|d>&recursive=<0|1>&with_thumbnail=<bool>`

移动端与网页界面使用的列表形状。

**响应 `200`**

```json
{
  "user_perm": "rw",
  "dir_id": "…",
  "dirent_list": [
    {
      "type": "dir",
      "id": "…",
      "name": "photos",
      "mtime": 1760000000,
      "permission": "rw",
      "parent_dir": "/",
      "starred": false
    },
    {
      "type": "file",
      "id": "…",
      "name": "notes.txt",
      "size": 1204,
      "mtime": 1760000000,
      "permission": "rw",
      "parent_dir": "/",
      "starred": true,
      "modifier_email": "user@example.com",
      "modifier_name": "User",
      "modifier_contact_email": "user@example.com"
    }
  ]
}
```

`user_perm` 等于每个条目的 `permission`（Nanofile 没有目录级分享）。带 `with_thumbnail=true` 时每个文件条目多一个 `encoded_thumbnail_src`，永远是空字符串；客户端把它当作「自己拼缩略图 URL」的信号。递归形状相同。这里也设置 `oid`/`dir_perm` 头，无提交资料库的 `dir_id` 用全零哨兵。

需要读权限与 `file.read`。

## 创建目录（v2.1） {#post-api-v2-1-repos-repo-id-dir}

`POST /api/v2.1/repos/{repo_id}/dir/`

**请求体**（JSON）—— `{"p": "/new/folder"}`，或把 `p` 放在查询参数。

**响应 `200`** —— `{"success": true}`。需要写权限与 `file.write`。

## 删除目录（v2.1） {#delete-api-v2-1-repos-repo-id-dir}

`DELETE /api/v2.1/repos/{repo_id}/dir/?p=<path>`

与旧路径同一操作与响应，包括 `reloaddir=true`。需要 `file.delete`。

## 目录详情 {#get-api-v2-1-repos-repo-id-dir-detail}

`GET /api/v2.1/repos/{repo_id}/dir/detail/?path=<path>`

一个目录的元数据。参数是 `path` 而不是 `p`，不接受根目录。

**响应 `200`**

```json
{ "repo_id": "…", "path": "/photos", "name": "photos", "mtime": 1760000000, "permission": "rw" }
```

`mtime` 是 Unix 秒（数字）。需要 `file.read`。

**Seafile** —— 路由相同；seahub 的 `mtime` 是 ISO 8601 字符串。Android 模型把它声明为数字，而且从不调用该接口。

## 重命名目录 {#post-api2-repos-repo-id-dir-rename}

`POST /api2/repos/{repo_id}/dir/rename/`

**请求体**（JSON）—— `{"repo_id": "…", "p": "/old/name", "new_name": "new-name"}`。资料库 id 在请求体里，与其他目录接口不同。

**响应 `200`** —— 空正文。需要写权限与 `file.write`。

## 移动目录 {#post-api2-repos-repo-id-dir-move}

`POST /api2/repos/{repo_id}/dir/move/`

**请求体**（JSON）—— `{"repo_id": "…", "p": "/old/name", "new_parent_dir": "/target"}`。同库内移动是一个提交。需要 `file.write`。

**响应 `200`** —— 空正文。

## 从目录创建资料库 {#get-api2-repos-repo-id-dir-sub-repo}

`GET /api2/repos/{repo_id}/dir/sub_repo/?p=<path>`

把一个目录提升为调用者拥有的新资料库，复制其树与块。由于是 `GET` 上的状态变更，浏览器会话必须带 `X-CSRFToken`；bearer 凭据不受影响。

**响应 `200`**

```json
{ "sub_repo_id": "…", "id": "…", "name": "photos", "desc": "", "size": 0, "encrypted": 0, "enc_version": 0, "owner": "user@example.com", "permission": "rw", "mtime": 1760000000 }
```

新资料库带一个提交。需要 `library.create` 与源资料库的写权限。

## 列出目录中的分享链接 {#get-api2-repos-repo-id-dir-shared-items}

`GET /api2/repos/{repo_id}/dir/shared_items/?p=<path>`

为恰好该目录路径创建的分享链接。

**响应 `200`**

```json
{
  "shared_items": [
    { "share_type": "download", "token": "…", "path": "/photos", "repo_id": "…", "creator_email": "", "created_at": 1760000000 }
  ]
}
```

需要 `file.read`。Nanofile 没有用户/群组分享，因此只有下载链接。

**Seafile** —— `dir/`、`dir/detail/`、`dir/shared_items/` 是官方记录的上游路由。`dir/rename/`、`dir/move/` 与请求体里的 `repo_id` 是 Nanofile 的便利接口；seahub 通过 `fileops` 与 `dir/` POST 重命名。
