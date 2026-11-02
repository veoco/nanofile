# Face-tag test fixtures

`portrait.jpg` is the official portrait of Barack Obama (Pete Souza, White
House), a work of the U.S. federal government and therefore **public domain**.
Source: <https://commons.wikimedia.org/wiki/File:President_Barack_Obama.jpg>.
It was resized to 512 px wide for the test and is used only to assert that
YuNet finds exactly one face.

`portrait.avif` is the same public-domain portrait, encoded as a still AVIF. It
exists because the `image` crate cannot decode AVIF (or HEIC/HEIF): the scan
decodes a still through the media profile with ffmpeg and then runs the detector.
Regenerate it with:

```bash
ffmpeg -y -i portrait.jpg -frames:v 1 -c:v libaom-av1 -still-picture 1 -crf 24 portrait.avif
```
