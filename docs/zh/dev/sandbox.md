# 沙盒内部

每个读取攻击者所选字节的解析器都在独立进程中运行：同一个二进制被重新执行为 `nanofile extract-worker`。子进程先自我约束，然后才从 stdin 读取请求。没有进程内路径：如果子进程起不来，或拒绝在没有约束的情况下运行，文档就是一次**失败**——稍后重试，绝不由服务器解析。

面向运维的行为与管理页见[沙盒](/zh/guide/sandbox)。

## Profile

profile 决定子进程可以建立哪些保护：

- `documents` —— 搜索索引的正文提取。
- `images` —— 图片解码与 EXIF/头像处理。
- `media` —— `ffmpeg` 缩略图，唯一会启动外部程序的 profile。

## worker 协议

- **请求**（stdin）：`NFX1`，一个字节命名 plan，随后是文件字节直到 EOF。
- **回复**（stdout）：`T` 或 `U`，一个 little-endian `u32` 长度，然后是同样长度的 UTF-8 字节——正文，或文档不可索引的原因。
- `--selftest` 用一行报告替代请求，不回复。
- `--probe` 运行服务进程启动时运行的 probe，worker 不可用时以非 0 退出。

父进程的 `accept` 在把子进程当作已约束之前检查报告。报告携带的是子进程**实际建立**的内容，而不是父进程要求的内容。

## 各 profile 建立什么

| 项 | 含义 | Linux | macOS | Windows |
|---|---|---|---|---|
| limits | 已提交内存、CPU 秒数；unix 上还包括句柄与文件大小 | `RLIMIT_AS` | `setrlimit`（映射空间加上限） | Job Object 内存与 CPU 上限 |
| files | 读写任何路径 | Landlock，零授权（媒体另有 helper、解释器与源文件） | Seatbelt profile | AppContainer |
| network | 创建 socket | seccomp 拒绝列表、Landlock TCP 权限 | Seatbelt profile | 无能力的 AppContainer |
| process | `exec`、`fork`、额外进程 | seccomp 拒绝列表（只对媒体 helper 与其加载器放行 `exec`） | Seatbelt profile：仅媒体 profile 放行 `exec`，解析器拒绝 `fork` 与 `exec` | Job 活动进程上限，加上内核的子进程策略 |

`limits` 是硬不变量。建立不了它的子进程会被父子两侧同时拒绝——父进程的 `accept` 与子进程读取请求前的自检。`files`、`network` 与 `process` 是纵深防御：平台给不了某一项时，功能只靠 `limits` 运行，报告会说明。

媒体 profile 不可能有 process 项：它的存在意义就是启动程序，而每个平台启动程序都要先复制进程。约束 helper 的是文件层与父进程的超时。

## 平台说明

- **Linux** —— 文件与 TCP 层用 Landlock，网络与进程层用 seccomp，边界用 `RLIMIT_AS`/`RLIMIT_CPU`/`RLIMIT_NOFILE`/`RLIMIT_FSIZE`。拒绝会被交给正在运行的内核验证：把过滤器装进一个子进程，再让它发起解析器绝不该发起的调用；每个调用都配一个同样进程、但不装过滤器的对照。
- **macOS** —— 每个子进程都在 `sandbox-exec` 的 Seatbelt profile 下运行。解析器 profile 拒绝 `fork`，并把 `exec` 收窄到子进程自身的镜像。
- **Windows** —— 先尝试 AppContainer，再叠加 Job Object、受限令牌与进程缓解策略。AppContainer 需要在注册表写 profile，并在 worker 自身镜像上给出包访问权限；一个约束自己所有子命令的主机会拒绝这些。此时启动报告 `files=open`，并且在该进程内不再申请容器。Job Object 约束已提交内存与 CPU 时间，但没有句柄或文件大小的等价物。

媒体 helper 有独立的文件授权：helper 与它的加载器、它旁边的库文件（逐文件）、系统库目录，以及在 macOS 上——仅当 helper 确实安装在该目录下时——它所在的打包管理器目录。在 Windows 上 helper 的标准流使用管道而不是空设备，因为容器可能拒绝打开该设备。

## 测试

沙盒是在使用它的地方被测的，而不只是在描述它的地方。

- `server/tests/sandbox_probe_test.rs` 断言每个 profile 的类型化报告，以及平台自身的机制 token。
- 每个 profile 还必须在它声称的约束下完成本职工作，判定依据是**产物**：probe 解析它携带的每一种结构化格式并比对解析器返回的全文；图片子进程的回复会对照交给它的像素；媒体子进程读取父进程写下的文件、解码一帧并交回编码后的缩略图。
- 文本接近上限的文档会在受限子进程里被完整抽出，因此资源限制是按"够用"来测的，而不只是"装上了"。
- worker 完全起不来的机器上，头像上传、EXIF 读取与缩略图都必须拒绝，而不是退回服务器进程内解码。

发布出去的二进制与镜像也会跑同一份 probe，那也是沙盒唯一一次在 `FROM scratch` 里运行。见[CI 与发布](/zh/dev/ci-releases)。

## 限制

沙盒不防的东西不会被断言为成立：内核漏洞，以及以同一用户运行的其他进程。这些测试针对的是本服务自己建立的约束，而不是它下面的内核。
