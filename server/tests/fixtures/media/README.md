# Media fixtures

`tiled.heic` is a synthetic HEIF tile grid, not a photograph: six 64×64 tiles
in three columns and two rows (192×128). The tiles are red, green and blue in
the first row, then yellow, magenta and cyan.

It exists because a phone HEIC is usually a **grid** image, and FFmpeg exposes
the grid as a `TILE_GRID` stream group rather than a single video stream. A
simple `-vf` chain is then rejected ("stream fed from a complex filtergraph"),
and on versions that accept it, the chain can return only the first tile. The
fixture makes both failures visible in one small file.

The file was encoded with libheif 1.23.7 (`heif_context_add_grid_image` plus
`heif_context_add_image_tile`, HEVC, quality 80). The tiles are 64 px because a
32 px tile is padded by the HEVC encoder to its minimum coded size; FFmpeg then
sees 64×64 tiles while the grid declares 32×32, and the assembled image does not
match the container. Any libheif build with the grid API can regenerate it; the
tile size is the part worth keeping at 64 px or more.
