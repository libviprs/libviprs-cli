<p align="center">
  <img src="https://raw.githubusercontent.com/libviprs/libviprs/main/images/libviprs-logo-claws.svg" alt="libviprs" width="200">
</p>

<h1 align="center">libviprs-cli</h1>

<p align="center">
  <img src="https://img.shields.io/badge/rust-1.85%2B-orange?logo=rust" alt="Rust 1.85+">
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

```bash
# Scanned blueprint PDF → DeepZoom PNG tiles
viprs pyramid blueprint.pdf output_tiles/

# With options
viprs pyramid blueprint.pdf tiles/ \
    --tile-size 512 \
    --overlap 1 \
    --layout xyz \
    --format jpeg \
    --quality 90 \
    --concurrency 8

# Vector PDF (requires libpdfium)
viprs pyramid autocad_export.pdf tiles/ --render --dpi 300

# With geo-referencing
viprs pyramid site_plan.pdf tiles/ \
    --geo-origin "-122.4194,37.7749" \
    --geo-scale "0.0001,-0.0001"

# Regular image files
viprs pyramid large_photo.tiff tiles/ --format png --concurrency 4
```

#### Options

| Flag | Default | Description |
|---|---|---|
| [`--tile-size`](https://libviprs.org/cli/#flag-tile-size) | 256 | Tile size in pixels |
| [`--overlap`](https://libviprs.org/cli/#flag-overlap) | 0 | Tile overlap in pixels |
| [`--layout`](https://libviprs.org/cli/#flag-layout) | deep-zoom | `deep-zoom` or `xyz` |
| [`--format`](https://libviprs.org/cli/#flag-format) | png | `png`, `jpeg`, or `raw` |
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

- Rust 1.85+
- libpdfium shared library (only for `--render` flag)

### PDFium setup

The `--render` flag requires `libpdfium.so` at runtime. Pre-compiled binaries are available from [libviprs-dep](https://github.com/libviprs/libviprs-dep/releases):

```bash
# x86_64
curl -L -o pdfium.tgz \
  https://github.com/libviprs/libviprs-dep/releases/download/pdfium-7725/pdfium-linux-x64.tgz

# arm64
curl -L -o pdfium.tgz \
  https://github.com/libviprs/libviprs-dep/releases/download/pdfium-7725/pdfium-linux-arm64.tgz

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
