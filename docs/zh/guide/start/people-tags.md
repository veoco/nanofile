# 自动人物标签

Nanofile 可以在不识别身份的前提下，给照片打上三种标签：

| 标签 | 含义 |
| --- | --- |
| 人物 | 至少有一张人脸 |
| 单人 | 恰好一张人脸 |
| 合影 | 两张或更多人脸 |

标签文字跟随界面语言显示；手动创建的标签保持你输入的名称。

这些标签可以搜索：搜 `人物`、`有人`、`people`、`face`、`单人`、`solo`、
`合影`、`合照` 或 `group` 都能找到对应文件。它们和普通标签一样，可以筛选、
追加其他标签，也可以移除。

## 保存了什么

只保存**人脸数量**和最高检测分数，以及每个文件的扫描状态。Nanofile **不会**
保存人脸裁剪、特征向量、关键点或身份模板，也无法回答“给我看某个人的照片”。
没有人物页面，也没有跨照片识别。模型是
[YuNet](https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet)，
MIT 许可；仓库里的 `server/models/MODEL_CARD.md` 有完整说明。

## 开启

所有构建都包含检测器，但扫描默认关闭，因为扫描图库会占用 CPU：

```toml
[face_tags]
enabled = true
backfill_enabled = true
min_score = 0.7
min_size = 32
```

也可以改用 `NANOFILE_FACE_TAGS_ENABLED`、
`NANOFILE_FACE_TAGS_BACKFILL_ENABLED`、`NANOFILE_FACE_TAGS_MIN_SCORE` 和
`NANOFILE_FACE_TAGS_MIN_SIZE`，或在
**系统设置 → 维护 → 自动人物标签** 中修改。`enabled` 和 `backfill_enabled`
重启后生效；两个阈值实时生效。

- `min_score`（0–1）是人脸需要达到的 YuNet 置信度。
- `min_size` 是**原图**中的人脸短边；更小的人脸忽略。想避免缩略图里的
  小人脸也被打标签，优先调高这一项。

重启后，新上传和同步的图片会在后台扫描。低负载补扫任务会以每轮 20 秒的
预算，扫描功能开启前已经存在的图片。

HEIC、HEIF、AVIF 照片同样会扫描：`image` crate 解不了它们，所以先用
`storage.ffmpeg_path` 解码出静帧，只把解出的图交给检测器。没有配置
`ffmpeg` 时这些文件会带原因跳过；配置好之后补扫会把它们捡回来。

## 纠正

从文件上移除自动标签会被记住：下次扫描不会再加回来。重新加回该标签会清除
这条纠正。如果文件内容变了（重新上传、同步修改），纠正也会清除，因为它描述
的是旧图片。

## 限制

- YuNet 大约能检测 10 px 以上的人脸，但默认 `min_size` 是 32 px，更小的人脸
  不计数。
- 大图会切成有重叠的瓦片，但人数很多、脸很小的合影仍可能漏检。
- 侧脸、口罩、严重遮挡和暗光会降低召回。
- HEIC/HEIF/AVIF 的静帧按长边不超过 4096 px 解码，因此更宽的图是按解码后的
  图来衡量。其他格式保留原始像素，上限是 `image` crate 的 8192 px。
- 模型在 WIDER Face 上训练，不同人群上的准确率并不一致。它只用于粗粒度的
  “有没有人脸”标签，不用于对某个人做判断。

想搜图片里的文字而不是图片里的人？见[图片文字识别](/zh/guide/start/ocr)。
