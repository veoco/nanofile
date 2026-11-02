# Automatic people tags

Nanofile can add three tags to photos without recognising anyone:

| Tag | Meaning |
| --- | --- |
| 人物 / People | At least one face |
| 单人 / Single person | Exactly one face |
| 合影 / Group photo | Two or more faces |

Tag labels follow the interface language. A tag you created yourself keeps the
name you typed.

The tags are searchable: searching for `人物`, `有人`, `people`, `face`, `单人`,
`solo`, `合影`, `合照` or `group` returns the matching files. They work exactly
like other tags — you can filter by them, add more tags, or remove one.

## What is stored

Only a **face count** and the best detection score are stored, next to the
per-file scan state. Nanofile does **not** create face crops, embeddings,
landmarks or identity templates, and it cannot answer "show me photos of this
person". There is no people page and no cross-photo recognition. The model is
[YuNet](https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet),
MIT-licensed; see `server/models/MODEL_CARD.md` in the repository.

## Turning it on

The feature is compiled into the release and nightly builds. It is off by
default because scanning a library costs CPU:

```toml
[face_tags]
enabled = true
backfill_enabled = true
min_score = 0.7
min_size = 32
```

Or set `NANOFILE_FACE_TAGS_ENABLED`,
`NANOFILE_FACE_TAGS_BACKFILL_ENABLED`, `NANOFILE_FACE_TAGS_MIN_SCORE` and
`NANOFILE_FACE_TAGS_MIN_SIZE` instead of editing the file, or use
**System Settings → Maintenance → Automatic people tags**. `enabled` and
`backfill_enabled` take effect after a restart; the two thresholds are live.

- `min_score` (0–1) is the YuNet confidence a face must reach.
- `min_size` is the short edge in the **original** image; smaller faces are
  ignored. Raising it is the quickest way to stop thumbnails of faces from
  producing tags.

After the restart, new uploads and syncs are scanned in the background. The
low-load backfill scans images that existed before the feature was enabled,
within a 20-second budget per pass.

## Corrections

Removing an automatic tag from a file is remembered: the next scan will not
add that tag back. Adding it back clears the correction. If the file's content
changes (a new upload, a sync edit), the correction is cleared too, because it
described the old image.

## Limits

- YuNet detects faces from roughly 10 px, but the default `min_size` is 32 px;
  faces below that are not counted.
- Large photos are split into overlapping tiles, but a very large crowd photo
  can still miss small faces.
- Side profiles, masks, heavy occlusion and poor light reduce recall.
- HEIC/HEIF/AVIF images are not face-scanned yet; they are skipped rather than
  retried.
- The model was trained on WIDER Face and its accuracy is not uniform across
  demographics. It is used for a coarse "there is a face" tag, not for a
  decision about a person.
