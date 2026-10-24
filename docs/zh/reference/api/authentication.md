# 认证

凭据、登录，以及客户端获取与丢弃凭据的接口。四种凭据类型与适用范围见[概述](/zh/reference/api/#authentication)。

源码：`server/src/handler/login.rs`、`server/src/handler/client_login.rs`、`server/src/handler/sso.rs`、`server/src/handler/device_wipe.rs`；测试：`server/tests/auth_test.rs`、`server/tests/client_compat_test.rs`。

## 获取账号令牌 {#post-api2-auth-token}

`POST /api2/auth-token/`

用账号的用户名与密码换取账号令牌。拿到后以 `Authorization: Bearer <token>`（或旧式 `Authorization: Token <token>`）发送到所有 API 接口。

**请求体** —— 默认 URL 编码表单；也接受 JSON 与 `multipart/form-data`，按 `Content-Type` 选择。

| 字段 | 类型 | 必填 | 含义 |
| --- | --- | --- | --- |
| `username` | string | 是 | 账号邮箱。 |
| `password` | string | 是 | 密码。 |
| `platform`、`device_id`、`device_name`、`client_version` | string | 否 | 记录到令牌上的设备信息，会话页会显示。 |

**两步验证** —— OTP 不在请求体里，而是走请求头，与 seahub 一致：

| 请求头 | 含义 |
| --- | --- |
| `X-SEAFILE-OTP` | 6 位 TOTP 验证码。 |
| `X-SEAFILE-S2FA` | 上一次成功登录返回的会话令牌，账号开启两步验证时使用。 |
| `X-SEAFILE-2FA-TRUST-DEVICE` | 设为 `1` 表示信任该设备 90 天。 |

**响应 `200`**

```json
{ "token": "…" }
```

请求要求信任设备（`X-SEAFILE-2FA-TRUST-DEVICE: 1`）且 OTP 正确时，响应还会带 `X-Seafile-S2FA` 头。保存该值，下次从同一设备登录时作为 `X-SEAFILE-S2FA` 发送即可跳过验证码。

**错误** —— `400`，DRF 的序列化错误形状：

```json
{ "non_field_errors": ["Unable to login with provided credentials."] }
```

其他消息覆盖账号禁用、邮箱未验证、尝试过多、缺少 OTP（同时设置 `X-Seafile-OTP: required` 头）与 OTP 无效。连续失败会让客户端地址在 `auth.lockout_duration_secs` 内被锁定。

**Seafile** —— 路由与令牌约定相同。Nanofile 额外接受 JSON 与 multipart 请求体（Android 客户端用 multipart）。

## 已认证的 ping {#get-api2-auth-ping}

`GET /api2/auth/ping/`

检查凭据是否有效。任何有效凭据都可以；响应体是 JSON 字符串 `"pong"`。

## 注销当前令牌 {#post-api2-logout-device}

`POST /api2/logout-device/`

使本次请求使用的账号令牌失效。凭据必须放在 `Authorization` 头里；只有会话 Cookie 时无法指明要吊销哪个令牌。

**响应 `200`** —— `{"success": true}`。

## 浏览器登录一次性令牌 {#post-api2-client-login}

`POST /api2/client-login/`

为桌面客户端的「在浏览器中打开」动作签发令牌。客户端随后打开 `{site_url}/client-login/?token=…` 完成浏览器登录。令牌绑定请求地址，30 秒后过期。

**响应 `200`** —— `{"token": "…"}`。要求账号凭据（会话或账号令牌）；不接受 API 密钥。

## 创建客户端 SSO 链接 {#post-api2-client-sso-link}

`POST /api2/client-sso-link/`

启动官方客户端使用的浏览器单点登录流程。该接口按协议匿名：移动端不带请求体和凭据，桌面端把设备信息放在查询参数里。

| 查询参数 | 含义 |
| --- | --- |
| `shib_platform` | 客户端平台，最长 128 字符。 |
| `shib_device_id` | 设备 id。 |
| `shib_device_name` | 确认页显示的名称。 |
| `shib_client_version` | 客户端版本。 |

**响应 `200`**

```json
{ "link": "https://files.example.com/client-sso/<token>/" }
```

链接始终使用配置的 `server.site_url`，从不使用请求的 `Host` 头。创建按客户端地址限流（`auth.sso_link_max_per_hour`）。

## 轮询客户端 SSO 链接 {#get-api2-client-sso-link-token}

`GET /api2/client-sso-link/{token}/`

轮询 SSO 链接状态。与创建接口一样是公开的。

**响应 `200`** —— 以下之一：

```json
{ "status": "waiting" }
{ "status": "success", "username": "user@example.com", "apiToken": "…" }
{ "status": "error" }
```

未知令牌返回 `404`。`success` 只会返回一次；客户端把 `apiToken` 存为账号令牌。

## 上报已擦除的设备 {#post-api2-device-wiped}

`POST /api2/device-wiped/`

被远程擦除的客户端上报自己，让服务器丢弃它持有的凭据。按协议公开，用被吊销的令牌自身认证：`Authorization: Bearer <token>`/`Token <token>`，或 URL 编码表单里的 `token` 字段（桌面客户端发这种）。

**响应 `200`** —— `{"success": true}`。

令牌必须是完整且未过期的账号令牌；处于两步验证等待状态的令牌会被拒绝。调用会吊销该设备的会话，以及该账号所有未使用的传输能力 URL（上传/下载链接）。

**Seafile** —— 路由相同；桌面端的表单字段与 Android 的 Authorization 头两种形状都接受。
