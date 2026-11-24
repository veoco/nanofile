# 沙盒内部

每个读取攻击者所选字节的解析器都在独立进程中运行：同一个二进制被重新执行为 `nanofile extract-worker`。子进程先自我约束，然后才从 stdin 读取请求。没有进程内路径：如果子进程起不来，或拒绝在没有约束的情况下运行，文档就是一次**失败**——稍后重试，绝不由服务器解析。

面向运维的行为与管理页见[沙盒](/zh/guide/admin/sandbox)。

## Profile

profile 决定子进程可以建立哪些保护：

- `documents` —— 搜索索引的正文提取。
- `images` —— 图片解码与 EXIF/头像处理。
- `media` —— `ffmpeg` 缩略图，唯一会启动外部程序的 profile。
- `faces` —— 自动人物标签的人脸检测。形状与 `images` 相同，但预算宽得多：检测器是一个模型 profile。
- `ocr` —— 全文索引的图片与 PDF 页文字的 PP-OCRv6 tiny 检测与识别，外加 PP-LCNet 页面与文本行方向分类器。形状与 `faces` 相同，预算最宽：四个模型，一次检测，每行文字一次识别；PDF 请求还会带页号，由 `hayro` 逐页渲染。

## worker 协议

- **请求**（stdin）：`NFX1`，一个字节命名 plan，随后是文件字节直到 EOF。模型任务有自己的帧：faces 在图片前多一个操作字节与 little-endian `u32` 选项字；OCR 的图片请求相同，PDF 请求再在 PDF 字节前加 `u32` 页数与每页 `u32` 页号。
- **回复**（stdout）：`T` 或 `U`，一个 little-endian `u32` 长度，然后是同样长度的 UTF-8 字节——正文，或文档不可索引的原因。PDF OCR 的 `T` 用 `\u{1e}` 分隔每个请求页的文字。OCR 另用 `E` 表示环境/模型失败，父进程把它记为可重试，而不是对输入内容的结论。
- `--selftest` 用一行报告替代请求，不回复。
- `--probe` 运行服务进程启动时运行的 probe，worker 不可用时以非 0 退出。

父进程的 `accept` 在把子进程当作已约束之前检查报告。报告携带的是子进程**实际建立**的内容，而不是父进程要求的内容。对 `media` 这一项检查更严：它还要求 `files` 与 `network` 两项，因为这个 profile 会启动外部程序。

## 各 profile 建立什么

| 项 | 含义 | Linux | macOS | Windows |
|---|---|---|---|---|
| limits | 已提交内存、CPU 秒数；unix 上还包括句柄与文件大小 | `RLIMIT_AS` | `setrlimit`（映射空间加上限） | Job Object 内存与 CPU 上限 |
| files | 读写任何路径 | Landlock，零授权（媒体另有 helper、解释器与源文件） | Seatbelt profile | AppContainer |
| network | 创建 socket | seccomp 拒绝列表、Landlock TCP 权限 | Seatbelt profile | 无能力的 AppContainer |
| process | `exec`、`fork`、额外进程 | seccomp 拒绝列表（只对媒体 helper 与其加载器放行 `exec`） | Seatbelt profile：仅媒体 profile 放行 `exec`，解析器拒绝 `fork` 与 `exec` | Job 活动进程上限，加上内核的子进程策略 |

`limits` 是硬不变量。建立不了它的子进程会被父子两侧同时拒绝——父进程的 `accept` 与子进程读取请求前的自检。

其余各层取决于该 profile 的本职工作：

- `documents`、`images`、`faces`、`ocr` 这些解析器 profile 只靠 `limits` 就运行。`files`、`network` 与 `process` 对它们是纵深防御：平台给不了某一项时，功能照常运行，报告会说明。
- `media` 只有在 **`files` 与 `network` 都就位**时才运行。它是唯一执行外部程序的 profile：没有把那个程序收窄到 helper 与源文件的层，也没有让它不至于变成通用程序的网络拒绝，这个 profile 就会被拒绝，而不是不带约束地运行。主机给不了这两层时（例如容器建不起来的受限 Windows 主机），媒体缩略图关闭，而解析器 profile 仍靠资源边界照常工作。

媒体 profile 不可能有 process 项：它的存在意义就是启动程序，而每个平台启动程序都要先复制进程。约束 helper 的是文件层与父进程的超时。

`faces` 与 `ocr` 是跑模型的 profile。faces 的墙钟是 90 秒，大图最多切成九个瓦片各做一次前向；OCR 的墙钟是 180 秒，加载一次四个模型，然后为页面方向、文本检测和每一行文字各跑一次前向；PDF 请求在 150 秒软截止处先返回已读的页。推理运行时是单线程的，所以 CPU 预算与墙钟相同（硬限制多 5 秒余量），真正的边界仍是墙钟超时。两者都不启动程序、不读文件：HEIC/HEIF/AVIF 先由媒体 profile 解码，只把解出来的静帧交给它们。

## 平台说明

- **Linux** —— 文件与 TCP 层用 Landlock，网络与进程层用 seccomp，边界用 `RLIMIT_AS`/`RLIMIT_CPU`/`RLIMIT_NOFILE`/`RLIMIT_FSIZE`。拒绝会被交给正在运行的内核验证：把过滤器装进一个子进程，再让它发起解析器绝不该发起的调用；每个调用都配一个同样进程、但不装过滤器的对照。
- **macOS** —— 每个子进程都在 `sandbox-exec` 的 Seatbelt profile 下运行。解析器 profile 拒绝 `fork`，并把 `exec` 收窄到子进程自身的镜像。
- **Windows** —— 先尝试 AppContainer，再叠加 Job Object、受限令牌与进程缓解策略。一个约束自己所有子命令的主机会拒绝容器，此时启动报告 `files=open`：解析器 profile 仍靠 Job Object 照常运行，媒体 profile 因为要求 `files` 与 `network` 会被拒绝，这类主机上媒体缩略图关闭。机制与回退见下文「Windows 约束」一节。

媒体 helper 有独立的文件授权：helper 与它的加载器、它旁边的库文件（逐文件）、系统库目录，以及在 macOS 上——仅当 helper 确实安装在该目录下时——它所在的打包管理器目录。在 Windows 上 helper 的标准流使用管道而不是空设备，因为容器可能拒绝打开该设备。

## Windows 约束

Windows 的约束是：子进程外层一个 Job Object，其下用一个受限令牌运行，再套一个 AppContainer，并加上内核愿意为该进程采取的进程缓解策略。

Windows 没有 `RLIMIT_AS`，也没有非特权的 Landlock，因此约束由若干层组成，父进程在创建时一次性施加：令牌、Job 与容器都是进程的属性，运行中的进程无法再采用。

* **Job Object**（父进程作为创建属性命名）：限制已提交内存、CPU 时间和进程数；父进程持有的句柄上的 `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` 会把子进程启动的东西一并带走。子进程读回限制而不是相信它们，自己也不持有句柄，因此内部无法提高限额。它不包含窗口站、剪贴板或句柄限制：这些从第一条指令起就生效，而 `JOB_OBJECT_UILIMIT_HANDLES` 会在加载器打印之前将其杀死。这里完全没有描述符或文件大小的边界。
* **受限令牌**（父进程在创建时施加）无法应用到运行中的进程，因此父进程用 `CreateProcessAsUser` 以自身令牌的受限版本启动子进程；这是 Windows 在无需 `SeAssignPrimaryToken` 时允许的唯一情形。特权被移除，管理 SID 变成 deny-only，写入按 restricting SID 检查。此处不设置完整性标签：低完整性是容器自身的属性，子进程报告它读回的级别（`il=`）。
* **AppContainer**（父进程在创建时施加）是进程创建属性：带 AppContainer SID 且 *没有能力* 的 `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES`。能力决定能否联网以及能否访问包外对象，因此空能力列表不会打开任何 socket，也不会读取 ACL 只写用户 SID 的文件。这是 Chromium 的零能力配置。

在此基础上，子进程设置它能接受的进程缓解策略——win32k 锁定、任意代码防护、无扩展点、无字体、无远程或低完整性镜像、ASLR 系列、严格句柄检查，以及除 media 外所有 profile 的子进程策略——并用 `GetProcessMitigationPolicy` 逐项读回。它们是 *运行时* 策略而非创建属性，因为 media profile 必须启动它的 helper。从创建到 `confine` 之间，Job 的活动进程上限是唯一限制子进程创建的因素，这段时间没有任何不受信内容运行。

restricting SID 是本进程自身身份：登录会话、用户，以及 Windows 为进程附着对象而授予的组（`INTERACTIVE`、`Authenticated Users`、`Users`、`Everyone`）。它们不能被去掉：每次访问都会对受限令牌做两次检查，restricting 列表缺少它们会让子进程虽已创建却死在加载器里，报 `STATUS_DLL_INIT_FAILED`（`0xC0000142`）。

因为用户自己的 SID 就在该列表中，`WRITE_RESTRICTED` 把写入收窄到该用户可写的位置，而不是收窄到零，单独看接近空操作。真正约束写入的是容器与完整性级别：AppContainer 检查要求在用户 SID 授予之外，*额外* 具备包 SID 或 `ALL APPLICATION PACKAGES` 的 ACE，因此只写用户 SID 的文件将不可读。worker 自身镜像必须保持可读，而按用户安装不会带这样的 ACE，所以父进程在首次启动前把它授予容器（`grant_image_access`）。

有容器时，四项都是平台自身提供的。没有容器时，子进程仍在 Job 中、以受限令牌和缓解策略运行，并报告 `files=open`：`limits` 是保证服务器不倒的边界，而容器是唯一能补上 files 与 network 两项的机制。每次启动只尝试一次；拒绝过的主机不会再被询问。

### 为什么主机可能没有容器

创建容器需要注册表 profile 以及 worker 自身镜像上的包 SID 访问权限，而一个约束自身所有命令的主机沙箱（开发或 agent 环境）恰好拒绝这些。失败表现为子进程死在加载器中且没有诊断，或 ACL 无法写入。因此启动先尝试容器，失败则退回 Job Object，并报告缺失的项。

### 受限能力容器（LPAC）

Windows 11 允许容器用 `PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY` 的 `..._OPT_OUT` 选择退出 `ALL APPLICATION PACKAGES`，`lpac=` 表示是否生效。Windows 10 会返回错误，因此该属性被丢弃、保留普通容器。该属性在创建时被接受，所以加载器无法在其中启动的容器会在 `CreateProcess` 返回成功之后才失败；父进程随即停止尝试，改用普通容器重来。

退出并不消除下面的说明。它是安全属性 `WIN://NOALLAPPPKG`，让低箱检查 *忽略* `ALL APPLICATION PACKAGES`，但并不把它从令牌中移除，进程仍是 AppContainer。让系统目录保持可读的是另一个主体：`ALL RESTRICTED APPLICATION PACKAGES`（`S-1-15-2-2`），其 ACE 随打包进程加载的系统文件一同发布——这是 Windows 构建的属性而非有文档保证的不变量，因此要测量 `system=`，而回答 `system=denied` 的主机是更严格而不是坏了。退出也不覆盖注册表：LPAC 需要 `registryRead` 能力才能读取解析器想要的键。

这里没有任何保护能约束边界的形状。容器读取它加载所需的系统目录（`Windows`、`Program Files`——其 ACL 带有低箱进程所需包的主体），与它被拒绝的用户路径一起报告为 `system=readable`。写入按实测报告——容器重定向存储内为 `writes=store`，之外为 `writes=user`——这里写入完全没有大小边界。它读取的注册表与触及的 IPC 同样走双主体检查，以及本进程递给它的句柄：平台自身的限制，与 macOS profile 的授权形状相同。

### media profile 的 helper 可以放在哪里

helper 需要显式授权——授权它自己的文件以及旁边的库，一次一个文件，且仅此而已——因为容器的文件访问由列举其自身包 SID 的 ACE 决定。子进程报告 `helper=allowed` 或 `helper-grant-refused`。

只授权文件是有意为之。向上逐级授权目录既无必要也有破坏性：`restricted_token` 保留 `SeChangeNotifyPrivilege`（`DISABLE_MAX_PRIVILEGE` 只关掉它以外的特权），持有它的调用者根本不做目录穿越检查，因此向上授权没有必要；而 `SetNamedSecurityInfo` 会“把当前继承模型强加到目标对象之下所有对象的 ACL 上”（Microsoft，《Automatic Propagation of Inheritable ACEs》），在 `C:\Users\<user>` 上加一条 ACE 就会改写其下所有对象的继承。

helper 无法运行时，stdio 是首要嫌疑：`NUL` 是容器可能拒绝打开的设备，而这个拒绝与进程创建来自同一次调用。流是管道——stdin 关闭，stdout 与 stderr 抽干——启动时报告 `parse=media-ok(<bytes>)`。只获得 `ALL APPLICATION PACKAGES` 授权的 helper，会为 media profile 放弃退出选项，而不是把它当作环境问题报告（`use_plain_container`）。

### 两个主体，以及为什么退出选项可以被放弃

低箱检查由进程包 SID 的 ACE 回答，或由 `ALL APPLICATION PACKAGES` 的 ACE 回答；系统目录携带的是后者，`dacl_grants_any_package` 走这条捷径，因此系统已经授权的文件不会在每次启动时被重写。退出选项让检查忽略它，因此该捷径只在本次启动不请求退出时才把读取视为通过。`add_access` 因此按启动自身的决定来询问；而本进程可能无权重写 DACL 的文件——系统自有的 DLL、`Program Files` 下的镜像——正是两者相遇之处：放弃退出选项、保留容器，而不是把一个读不了的镜像交给子进程。启动报告 `lpac=off`，设置页显示提示，日志说明 media profile 放弃了它。

### 参考

* Microsoft，《Launch an AppContainer》：该属性、空能力列表，以及镜像必须对容器可读的要求。
* Microsoft，《Process Mitigation Policies》与《Job Objects》，包括 `PROC_THREAD_ATTRIBUTE_JOB_LIST` 以及只对 Job 内进程接受的子进程策略。
* Chromium 的 `sandbox/win/src/`：同样的启动方式、同样的单活动进程 Job，以及“创建时子进程策略存在 *是因为* Job 可以被逃逸”的说明。

## 测试

沙盒是在使用它的地方被测的，而不只是在描述它的地方。

- `server/tests/sandbox_probe_test.rs` 断言每个 profile 的类型化报告，以及平台自身的机制 token。
- 每个 profile 还必须在它声称的约束下完成本职工作，判定依据是**产物**：probe 解析它携带的每一种结构化格式并比对解析器返回的全文；图片子进程的回复会对照交给它的像素；媒体子进程读取父进程写下的文件、解码一帧并交回编码后的缩略图；faces 子进程加载模型并完整跑一次检测，报告 `faces-ok(0)`（检测到人脸的正确性由内置人像单测覆盖）。
- 文本接近上限的文档会在受限子进程里被完整抽出，因此资源限制是按"够用"来测的，而不只是"装上了"。
- worker 完全起不来的机器上，头像上传、EXIF 读取与缩略图都必须拒绝，而不是退回服务器进程内解码。
- 媒体 profile 的额外要求从两侧测试：缺少 `files` 或 `network` 的合成报告会被拒绝并点名缺失的层；给不了这些层的主机会让媒体相关测试按该原因跳过而不是失败。同一主机上的解析器 profile 仍会被测量并照常运行。

发布出去的二进制与镜像也会跑同一份 probe，那也是沙盒唯一一次在 `FROM scratch` 里运行。见[CI 与发布](/zh/dev/ci-releases)。

## 限制

沙盒不防的东西不会被断言为成立：内核漏洞，以及以同一用户运行的其他进程。这些测试针对的是本服务自己建立的约束，而不是它下面的内核。
