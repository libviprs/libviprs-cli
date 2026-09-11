<p align="center">
  <img src="https://raw.githubusercontent.com/libviprs/libviprs/main/images/libviprs-logo-claws.svg" alt="libviprs" width="200">
</p>

<h1 align="center">libviprs-cli</h1>

<p align="center">
  <img src="https://img.shields.io/badge/rust-1.97%2B-orange?logo=rust" alt="Rust 1.97+">
  <img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT License">
</p>

Command-line interface for [libviprs](../libviprs), a pure-Rust image pyramiding engine.

## Documentation

Full reference, including a flag-by-flag interactive program generator, lives at <https://libviprs.org/cli/>.

## Installation

```bash
cargo install --path .
```

## Commands

Each command below links to its section on the CLI docs page. Flag rows link to per-flag anchors with longer descriptions, defaults, and worked examples.

### [`viprs pyramid`](https://libviprs.org/cli/#pyramid)

Generate a tile pyramid from a PDF or image file.

Since 0.4.0 a pyramid goes into **one PMTiles v3 archive** by default, not a directory of millions of loose files. The output argument is optional: with none, the archive takes the input's name.

```bash
# Scanned blueprint PDF → blueprint.pmtiles
viprs pyramid blueprint.pdf

# Name the archive yourself
viprs pyramid blueprint.pdf drawings/plan-01.pmtiles

# The loose {z}/{x}/{y} tree earlier versions wrote
viprs pyramid blueprint.pdf tiles/ --storage directory

# With options
viprs pyramid blueprint.pdf plan.pmtiles \
    --tile-size 512 \
    --overlap 1 \
    --format jpeg \
    --quality 90 \
    --concurrency 8

# Vector PDF (requires libpdfium)
viprs pyramid autocad_export.pdf --render --dpi 300

# With geo-referencing
viprs pyramid site_plan.pdf \
    --geo-origin "-122.4194,37.7749" \
    --geo-scale "0.0001,-0.0001"

# Regular image files
viprs pyramid large_photo.tiff --format png --concurrency 4
```

#### Coming from 0.3.x

`viprs pyramid drawing.tif tiles` used to mean a directory called `tiles`. It now refuses with exit 2 rather than writing an archive called `tiles`, and names the flag that restores the old behaviour. Three combinations are refused the same way, each because it worked before the flip and has no meaning after it:

| What you typed | Why it stops | What to type instead |
|---|---|---|
| a directory-shaped output (existing directory, trailing `/`, or no extension) | the default target is one archive file | `--storage directory`, or name the archive `tiles.pmtiles` |
| `--layout deep-zoom` | PMTiles v3 addresses tiles as slippy `z/x/y`, and a Deep Zoom tier is not a slippy zoom | `--storage directory`, or drop `--layout` |
| `--format raw` | a PMTiles tile is a blob a viewer hands to a decoder, and raw pixels carry neither dimensions nor pixel format | `--format png`, `--format jpeg`, or `--storage directory` |

`--layout` has no single default any more: an archive gets `xyz`, a directory gets `deep-zoom`.

#### Options

| Flag | Default | Description |
|---|---|---|
| [`--storage`](https://libviprs.org/cli/#flag-storage) | pmtiles | `pmtiles` (one archive) or `directory` (a loose tile tree) |
| [`--tile-size`](https://libviprs.org/cli/#flag-tile-size) | 256 | Tile size in pixels |
| [`--overlap`](https://libviprs.org/cli/#flag-overlap) | 0 | Tile overlap in pixels |
| [`--layout`](https://libviprs.org/cli/#flag-layout) | storage-dependent | `deep-zoom`, `xyz` or `google` |
| [`--format`](https://libviprs.org/cli/#flag-format) | png | `png`, `jpeg`, or `raw` (`raw` needs `--storage directory`) |
| [`--quality`](https://libviprs.org/cli/#flag-quality) | 85 | JPEG quality (1-100) |
| [`--dpi`](https://libviprs.org/cli/#flag-dpi) | 150 | PDF rasterization DPI |
| [`--page`](https://libviprs.org/cli/#flag-page) | 1 | PDF page number (1-based) |
| [`--concurrency`](https://libviprs.org/cli/#flag-concurrency) | 0 | Worker threads (0 = single-threaded) |
| [`--geo-origin`](https://libviprs.org/cli/#flag-geo-origin) | | Geo origin as `"lon,lat"` |
| [`--geo-scale`](https://libviprs.org/cli/#flag-geo-scale) | | Pixel scale as `"sx,sy"` (degrees/pixel) |
| [`--render`](https://libviprs.org/cli/#flag-render) | off | Use PDFium for vector PDF rendering |

See the [pyramid command page](https://libviprs.org/cli/#pyramid) for the complete flag list (including `--memory-budget` and other tuning knobs) and an interactive Rust program generator.

### [`viprs info`](https://libviprs.org/cli/#info)

Show information about a PDF or image file.

```bash
$ viprs info blueprint.pdf
PDF: blueprint.pdf
Pages: 1
  Page 1: 3370.0 x 4768.0 pts (has images)

$ viprs info photo.png
Image: photo.png
Dimensions: 4096x3072
Format: Rgb8
Size: 36.0 MB
```

### [`viprs plan`](https://libviprs.org/cli/#plan)

Preview the pyramid layout (level count, tile counts, output bytes) without writing tiles. See the [plan command page](https://libviprs.org/cli/#plan) for flags and example output.

### [`viprs test-image`](https://libviprs.org/cli/#test-image)

Generate synthetic test images (gradients, checkerboards, noise) for benchmarking and fixture creation. See the [test-image command page](https://libviprs.org/cli/#test-image) for flags and example output.

### [`viprs pmtiles`](https://libviprs.org/cli/#pmtiles)

Inspect, read and unpack a PMTiles v3 archive. A container utility rather than a vips operation, so it is a first-class command and is not in `OP_MAP.md`.

```bash
# What is in here
$ viprs pmtiles info plan.pmtiles
Archive: plan.pmtiles
Version: 3
Tile type: png
Tile compression: none
Internal compression: gzip
Clustered: yes
Zoom: 0-10
Addressed tiles: 17
Tile entries: 17
Unique payloads: 17
Root entries: 17
Leaf directories: no
Archive size: 278679 bytes
Bounds: -180.000000,-85.051129,180.000000,85.051129
Center: 0.000000,0.000000 zoom 5
Metadata: {"name":"plan","vnd.libviprs":{ ... }}

# One tile, raw bytes on stdout and nothing else
viprs pmtiles tile plan.pmtiles 10 1 0 > tile.png
viprs pmtiles tile plan.pmtiles 10 1 0 --output tile.png

# Header and directory validation, non-zero exit on corruption
viprs pmtiles verify plan.pmtiles

# Back to a loose {z}/{x}/{y}.png tree
viprs pmtiles extract plan.pmtiles ./tiles
```

`tile` writes the tile and only the tile to stdout; every diagnostic, including the one for a tile that is not in the archive, goes to stderr. An absent tile exits 1 with nothing on stdout, so piping into a decoder cannot pick up a sentence where the bytes should be.

`verify` is deliberately stricter than the reference `go-pmtiles` implementation in one place: it checks every entry's `offset + length` against the section that owns it, which go-pmtiles' own `verify` never does.

`extract` reproduces exactly what `viprs pyramid --storage directory --layout xyz` writes for the same input, so an archive somebody hands you turns into the tree existing tools already serve. Tiles stored under a deduplicating run get written out once per coordinate.

There is no `webp` in `--format`, and there will not be one until the core crate's `TileFormat` grows the variant. PMTiles v3 defines a WebP tile type and `viprs pmtiles info` reports it when somebody else's archive carries one, but nothing this CLI writes can contain one and the flag is not going to say otherwise.

## PDF Handling

The CLI supports two modes for PDF input:

**Default (lopdf extraction):** Extracts embedded raster images directly from the PDF stream. Fast, no external dependencies. Best for scanned blueprints where the PDF is a wrapper around a JPEG.

**[`--render`](https://libviprs.org/cli/#flag-render) (PDFium):** Renders the PDF page to a bitmap at the specified DPI. Required for vector PDFs (AutoCAD exports, text, paths). Needs libpdfium installed on the system.

## Development

### Git Hooks

Install pre-commit (fmt + clippy) and pre-push (Docker test suite) hooks:

```bash
../libviprs-tests/tools/install-hooks.sh
```

## Requirements

- Rust 1.97+
- libpdfium shared library (only for `--render` flag)

### PDFium setup

The `--render` flag requires `libpdfium.so` at runtime. Pre-compiled binaries are available from [libviprs-dep](https://github.com/libviprs/libviprs-dep/releases):

```bash
# x86_64
curl -L -o pdfium.tgz \
  https://github.com/libviprs/libviprs-dep/releases/download/pdfium-7881/pdfium-linux-x64.tgz

# arm64
curl -L -o pdfium.tgz \
  https://github.com/libviprs/libviprs-dep/releases/download/pdfium-7881/pdfium-linux-arm64.tgz

# Extract and install
tar xzf pdfium.tgz
sudo cp pdfium-linux-*/lib/libpdfium.so /usr/local/lib/
sudo ldconfig
```

See the [libviprs-dep pdfium README](https://github.com/libviprs/libviprs-dep/tree/main/pdfium) for building from source or other versions.

## Related Crates

| Crate | Description |
|---|---|
| [libviprs](../libviprs) | Core library |
| [libviprs-tests](../libviprs-tests) | Integration tests and fixtures |
