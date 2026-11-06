# OCR fixtures

`sample.png` is the sample page shipped with tract's PP-OCRv6 example:

- Source: <https://github.com/sonos/tract/tree/v0.23.10/examples/ppocr/web/site/sample.png>
- The example describes it as a sample page from the PP-OCRv6 model card.
- Licence: the tract repository is MIT OR Apache-2.0; this file is used only as a
  test fixture and is not embedded in the server binary.
- SHA-256: `244cf16cfd20d5582ba4cfb25b3e2655050af2e3bb029e1d5c59698533209ca1`

The end-to-end OCR test expects the recognised text to contain
`Markov Entropy Decomposition` and at least 20 non-empty lines. If the fixture
is replaced, update those assertions and the hash above.

`cargo test` reads the file from `server/tests/fixtures/ocr/`; nothing downloads
it at runtime.
