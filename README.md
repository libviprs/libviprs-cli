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

### Cargo features

`viprs` forwards every capability-bearing feature of the core crate under the same name. Only `pdfium` is on by default.

| Feature | What it turns on |
|---|---|
| `pdfium` (default) | `--render` for vector PDFs, binding `libpdfium.so` at runtime |
| `pdfium-static` | PDFium linked at build time instead; needs a static `libpdfium.a` and `PDFIUM_STATIC_LIB_PATH` |
| `avif` | AVIF decoding |
| `svg` | SVG rasterising |
| `jxl` | JPEG XL decoding |
| `jp2k` | JPEG 2000 decoding |
| `object-store-sink` | the core's injected-backend object-store sink |
| `s3` | the `s3://` sink arm (the core treats `s3` as a deprecated alias for `object-store-sink`) |
| `packfile` | the `packfile://` tar/zip sink |
| `tracing` | `--trace-level` span output |
| `full` | all of the above except `pdfium-static` |

```bash
# Everything a runtime-pdfium build can have
cargo install --path . --features full

# One extra decoder
cargo install --path . --features avif
```

A format whose feature was left out is refused with exit 1 and a message naming the feature to rebuild with, never a generic "unsupported format". [`viprs features`](#viprs-features) says what a given binary has.

## Exit codes

One contract for every command, built-ins and ops alike:

| Code | Meaning |
|---|---|
| `0` | Success. |
| `1` | Operational failure: an input that cannot be read or decoded, an I/O error, a check that found a problem, or a feature this build left out (the message names the `--features` flag to rebuild with). |
| `2` | Usage mistake: an unknown flag, a missing or conflicting argument, a value out of range, or a combination of flags that has no meaning. Nothing is read or written. |
| `130` | Interrupted by Ctrl-C (SIGINT). A process killed by the signal is reported as 130 by the shell too. |

A missing feature is a 1 rather than a 2 on purpose: the command line was fine, this binary just cannot do it, and rebuilding fixes it where retyping would not.

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

#### Pipeline controls

`--layout` picks the pyramid scheme. It is not `--pmtiles-layout`, which only decides where the tile bytes sit inside an archive.

| Flag | Applies to | Description |
|---|---|---|
| `--pmtiles-layout tile-id\|arrival` | archive | `tile-id` (default) sorts at the end and is always clustered; `arrival` writes every byte once, in the order tiles came |
| `--ordered-emission` | archive | Emit tiles in tile id order, so an `arrival` archive is byte-identical to the `tile-id` one |
| `--dedupe-memory-bytes N` | archive | Writer's duplicate-tile window, about 65 bytes a tile; below 520 is refused, and above a slot for every planned tile it is lowered to that (the run says so) |
| `--resume` / `--checkpoint-every N` / `--checkpoint-root DIR` | tree | Pick up an interrupted run; checkpoint every 1000 tiles by default |
| `--retries N` / `--retry-backoff-ms MS` | all | Retry a failed write up to N times, then fail the run |
| `--skip-failed` | all | Skip a tile that still fails and carry on; the run then exits 1 at the end, since the output has holes |
| `--fail-fast` | all | Abort on the first failure (the default) |
| `--checksum` / `--manifest-source-hash` | tree | Per-tile checksums (re-hashed before the run succeeds) and the BLAKE3 of the source file's bytes, in `manifest.json` |
| `--region x,y,w,h` | all | Crop, then pyramid |
| `--drop-blanks` | all | Leave blank tiles out altogether (`--skip-blank` writes a placeholder per blank tile instead; the two can't be combined) |
| `--events none\|text\|json` | all | One line per engine event on stdout; each `json` line has `"v":1` and an `"event"` name |

`--memory-budget` streams a fresh run into a tile tree. Into an archive or an object store, or with `--resume` or `--verify`, the run uses the monolithic engine and says so on stderr. `--trace-level` output goes to stderr too, so stdout carries nothing but `--events`.

Ctrl-C stops any pyramid run at the next tile and exits 130. A tile tree it leaves behind finishes with the same command plus `--resume`, to exactly the bytes an uninterrupted run writes.

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

`--json` prints the same as one JSON object, with the exact decoded size in `bytes`:

```bash
$ viprs info --json photo.png
{"v":1,"kind":"image","path":"photo.png","width":4096,"height":3072,"format":"Rgb8","bytes":37748736}

$ viprs info --json blueprint.pdf
{"v":1,"kind":"pdf","path":"blueprint.pdf","pages":1,"page_sizes":[{"page":1,"width_pts":3370.0,"height_pts":4768.0,"has_images":true}]}
```

### [`viprs plan`](https://libviprs.org/cli/#plan)

Preview the pyramid layout (level count, tile counts, output bytes) without writing tiles. See the [plan command page](https://libviprs.org/cli/#plan) for flags and example output.

`plan` also answers one planner question at a time and prints only the answer, so it can be captured as is:

```bash
viprs plan 5000 --height 3000 --estimate-memory 256              # streaming peak memory in bytes for a 256 row strip
viprs plan 5000 --height 3000 --dzi-manifest=png                 # the .dzi XML (deep-zoom only; a bare --dzi-manifest means png)
viprs plan 5000 --height 3000 --layout zoomify --properties-sidecar png   # ImageProperties.xml (zoomify) or info.json (iiif)
viprs plan 5000 --height 3000 --tile-path 13,3,2 --tile-ext jpg  # where one tile goes, as LEVEL,COL,ROW
viprs plan 5000 --height 3000 --overlap 4 --tile-rect 13,3,2     # x,y,width,height that tile reads from
```

`plan --layout` takes `zoomify` and `iiif` as well as the three `pyramid` writes, so those sidecar and tile-path questions can be asked. `pyramid --layout` stays `deep-zoom`, `xyz` or `google`.

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

### `viprs features`

Print the cargo features this binary was built with, one per line, in alphabetical order. `--json` prints `{"features": [...]}` instead. It always exits 0, and a `--no-default-features` build prints nothing.

```bash
$ viprs features
pdfium

$ viprs features --json
{"v":1,"features":["pdfium"]}
```

### Loading and saving every codec

Every codec libviprs ships has a `*save` and a `*load` command, spelled the way vips spells it, so `vips jpegsave in.png out.jpg --Q 90` becomes `viprs jpegsave in.png out.jpg --Q 90`.

| save | options | load | options |
|---|---|---|---|
| `jpegsave` | `--Q`, `--subsample-mode auto\|on\|off` | `jpegload` | `--shrink 1\|2\|4\|8` |
| `pngsave` | `--compression 0-9`, `--interlace`, `--palette`, `--bitdepth` (with `--palette`) | `pngload` | |
| `tiffsave` | `--compression none\|lzw\|deflate` | `tiffload` | `--page`, `--max-pages` |
| `webpsave` | `--lossless` (required) | `webpload` | `--page`, `--n` |
| `gifsave` | `--dither`, `--bitdepth`, `--interlace` | `gifload` | `--page`, `--n`, `--max-pages` |
| `jxlsave` | `--lossless` (required) | `jxlload` | |
| `jp2ksave` | `--lossless` (required), `--tile-width`, `--tile-height` | `jp2kload` | |
| `fitssave` | | `fitsload` | |
| `radsave` | | `radload` | |
| `uhdrsave` | `--Q`, `--gainmap-scale-factor` | `uhdrload` | |
| `csvsave` | | `csvload` | |
| `matrixsave` | | `matrixload` | |
| `ppmsave` | | `ppmload` | |

Plus `heifload` (AVIF only), `svgload` (`--dpi`, `--scale`, `--unlimited`), `openexrload`, `niftiload`, `analyzeload` and `matload`. JPEG XL, JPEG 2000, AVIF and SVG need their cargo feature; without it the command refuses and names the feature to rebuild with.

Every loader takes the five `--max-*` decode limits, and `-` reads the image from stdin (except `analyzeload`, whose image is a file pair). `webpsave`, `jxlsave` and `jp2ksave` only write lossless, while vips defaults to lossy for all three, so they insist on `--lossless` rather than quietly writing something other than what the vips spelling asks for.

The op commands also pick the encoder from the output extension, so `viprs copy in.png out.webp` works for `.webp`, `.gif`, `.jxl`, `.jp2`, `.fits`, `.hdr`, `.csv` and `.mat` as well as `.png`, `.tif`, `.ppm` and `.v`. `.jpg` is the exception: it stays banned there, because those commands are the ones the vips differential suite compares, and `jpegsave` is the way to write one.

Some things vips has are not here because the library itself refuses them: JPEG restart markers, tiled, BigTIFF and multi-page TIFF writing, and HEIF/HEIC, OpenSlide, ImageMagick and `dzsave`. `OP_MAP.md` says why for each.

### `viprs pdf`

`viprs pdf info FILE` lists pages and sizes, `viprs pdf rotation FILE --page N` prints a page's `/Rotate`, and `viprs pdf extract FILE OUT` writes one page as an image. `--dpi`, `--background R,G,B[,A]` (each channel 0 to 255) and `--render-budget PIXELS` render through PDFium, so they need a default-features build and a libpdfium (see below).

With no render option, `extract` pulls the page's largest embedded image at its stored size. Encryption changes that only when the file needs a password to open:

- An unencrypted file, or one with only an owner password (it opens without one and the owner password just restricts printing or copying), gives the embedded image at its stored size. For an owner-only file under AES-256 that takes PDFium, which decodes the image to 8 bits per sample.
- A file that needs a user password is opened with it and the page is **rendered** at 72 DPI (one pixel per point, so an A4 page comes out 595x841), because its streams can't be read without decrypting. A 300 DPI scan comes back at 72 DPI on this route.

A file that needs a password and gets none exits 1 saying a password is needed, and a wrong one exits 1 saying "wrong password". Both messages come from the library's typed errors, never echo the password, and a build without PDFium gets the library's own "not available in this build" instead. A password can't be combined with a render option, because the render path takes none.

The password comes from, in order:

1. `--password-file PATH`, or `--password-file -` to read it from stdin. One trailing newline is dropped.
2. `--password TEXT`. Anything on the command line shows up in `ps` and `/proc` for every user of the machine and stays in shell history, so prefer the other two outside a throwaway shell.
3. The `VIPRS_PDF_PASSWORD` environment variable, used when neither flag is given (an empty value counts as unset).

`--password` and `--password-file` can't both be given (exit 2).

### `viprs geo`

`viprs geo pixel-to-geo X Y`, `geo-to-pixel X Y` and `tile-center COL ROW --tile-size N` map points through a transform given as `--geo-origin X,Y --geo-scale X,Y` (the same pair `viprs pyramid` takes, through the same parser) or as the six affine coefficients `--affine a,b,c,d,e,f`. Each prints `x,y`; `geo-to-pixel` exits 1 for a transform that cannot be inverted. A value that isn't a finite number (NaN, `inf`, or something like `1e400` that overflows) is a usage error, exit 2, on `geo` and `pyramid` alike. So is a value that starts with `--`, which is a forgotten value swallowing the next flag (a negative number like `-122.4,37.7` is fine, spaced or joined with `=`), and on `pyramid` either of `--geo-origin` / `--geo-scale` without the other.

### `viprs verify`

Check a finished pyramid and name any tile that is missing or damaged. Exit 0 with a summary, or exit 1 with one line per bad tile.

```bash
# An archive: plan, structure, every tile present and decoded
viprs verify blueprint.pmtiles

# A tree written with --checksum: every tile against its recorded checksum
viprs verify tiles/

# Re-render from the input and compare (byte for byte for --format raw trees)
viprs verify tiles/ --source blueprint.png
```

The manifest and the archive metadata don't record `--centre` or `--drop-blanks` yet, so a pyramid written with either needs the same flag on `verify`. Without it a centred pyramid gets checked against the uncentred grid, and every dropped blank shows up as a missing tile (the message says so). `--drop-blanks` can't be combined with `--source`, because the re-render expects every planned tile, and a PDF can't be a `--source`, because verify doesn't know the page, DPI or render mode the pyramid used. `--source` decodes through the same input path as every other command. A badly broken pyramid is reported up to 50 problems, then verify stops looking.

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
