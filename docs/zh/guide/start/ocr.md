# 图片文字识别（OCR）

Nanofile 可以读取图片中的文字并写入全文索引：截图、拍到的招牌或扫描页，都能用图中出现的词搜到，而不只是靠文件名。该功能默认关闭。

## 开启

OCR 服务于全文索引，所以**必须先开启搜索索引**（`[index] enabled`）。然后：

```toml
[ocr]
enabled = true
min_score = 0.55
max_side = 960
```

同样的键在**系统设置 → 维护 → 图片文字识别（OCR）**，或环境变量 `NANOFILE_OCR_ENABLED`、`NANOFILE_OCR_MIN_SCORE`、`NANOFILE_OCR_MAX_SIDE`。它们无需重启即可生效。配置文件也接受 `[ocr]` 作为启动值，但设置页才是参考。

- `min_score`（0–1）：一行文字达到该识别置信度才会写入索引。调高可过滤噪声行，调低可保留较淡的印刷字。
- `max_side`：检测器把图片缩放到的最长边，向上取 32 的倍数（320–1600）。值越大越能看清小字，但更耗 CPU。

新上传和同步的图片会在后台处理；已有图片由低负载的 backfill 补齐。之后才开启 OCR 不需要重建整个索引。backfill 对每张图片只检查一次，因此没有文字的图片不会被每一轮反复读取。

## 会保存什么

只保存识别出的**文字**，作为该文件的正文进入全文索引。不按文件保存图片裁剪、文本框、置信度或模型状态。

索引目录**不加密**，文字里可能包含姓名、地址或证件号码。能读取该资料库的人就能搜到这些文字；资料库权限本身不变。关闭 `[ocr]` 只停止新的 OCR，已经写入索引的文字会保留到重建索引为止（重新索引资料库，或调用 `POST /api2/reindex/`）。

## 格式与上限

- PNG、JPEG、WebP、BMP、GIF、TIFF、ICO 由 OCR worker 解码。
- HEIC、HEIF、AVIF 先经配置的 `storage.ffmpeg_path` 解出一帧；没有 `ffmpeg` 时这些文件不做 OCR。
- 单张图片最多 32 MiB，最多识别 300 行文字；文字上限与文档提取相同，为 8 MiB。
- 手写、明显旋转、很小的字、低对比度照片和装饰字体都会降低准确率。
- 多栏页面按简单的自上而下顺序读取。搜索不受影响，但高亮片段里的顺序可能与原页面不同。
- 扫描版 PDF 和视频**不会**做 OCR。PDF 的文字仍来自 PDF 的文本层。

## 搜索

搜索行为不变：OCR 文字就是文件的正文，按内容搜索会命中，并在结果里给出高亮片段。文件详情里的「全文索引」区域显示同一段文字。标签、文件名和权限都不变。

## 模型与许可

PP-OCRv6 tiny（检测 + 识别）在受限的 OCR worker 中通过纯 Rust 的 `tract` 运行时执行。模型编译进二进制，运行时不下载。模型为 Apache-2.0；来源、revision 和限制见仓库里的 `server/models/MODEL_CARD.md`。

## 不是 Seafile 的 OCR 接口

Seafile 的 per-repo OCR metadata 接口没有实现。本页描述的是 Nanofile 自己的全文索引所用的自动 OCR；它不会打开 Seafile 的 OCR 功能，也不会创建 Seafile 的 metadata 记录。

## 故障排查

- **图片搜不到。** 确认 `index.enabled` 与 `ocr.enabled` 都开着，且沙盒开着。见[沙盒](/zh/guide/admin/sandbox)：OCR 那一组会显示本机能否加载模型。
- **HEIC/AVIF 没有被读取。** 配置 `storage.ffmpeg_path`；helper 可用后 backfill 会重试这些文件。
- **整体很慢。** 调低 `max_side`、调高 `min_score`，或关闭 OCR。OCR worker 与文档解析、缩略图相互独立，运行期间不会占用它们的许可。
- **关闭 OCR 后文件仍显示旧文字。** 文字在索引里，重新索引该资料库后才会消失。
