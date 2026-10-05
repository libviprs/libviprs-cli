# Changelog

Notable changes to `viprs` land here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
the `version` in `Cargo.toml`.

## [Unreleased]

### Added

- `viprs pmtiles pack TREE ARCHIVE`, the inverse of `extract`. The plan comes
  from a manifest or from explicit flags and is never guessed, the effective
  plan is printed before packing, and a pack that finds no tiles exits 1
  instead of writing an empty archive (#59).
- Cargo features `avif`, `svg`, `jxl`, `jp2k`, `pdfium-static`,
  `object-store-sink` and `full`, each forwarding to the core feature of the
  same name, and `viprs features [--json]` to list what a binary was built
  with (#64).
- A `*save` and a `*load` command for every codec libviprs ships, spelled
  the way vips spells them (`jpegsave`, `webpload`, `svgload` and the rest),
  with the codec options the core can honour as flags. Every loader takes
  the five `--max-*` limits and reads stdin for `-` (#65).
- The op commands write `.webp`, `.gif`, `.jxl`, the JPEG 2000 suffixes,
  `.fits`, `.hdr`, `.csv` and `.mat` through the core's own extension table.
  A `.jxl` or `.jp2` save in a build without that feature is refused, naming
  the feature (#65).
- `sobel`, `scharr`, `prewitt`, `canny`, `matrixmultiply`, `remainder` and
  `join` (#67).
- `viprs pdf info|rotation|extract`, `viprs geo pixel-to-geo|geo-to-pixel|tile-center`
  and the `viprs plan` query flags (`--estimate-memory`, `--dzi-manifest`,
  `--properties-sidecar`, `--tile-path`, `--tile-rect`). `pdf info` and
  `pdf extract` open an encrypted file with a password from
  `--password-file PATH` (`-` for stdin), `--password` or
  `VIPRS_PDF_PASSWORD`. `pdf extract` with no render option gives the
  embedded image at its stored size, except for a file that needs a user
  password, whose page is rendered at 72 DPI. `plan --layout` also takes
  `zoomify` and `iiif`; `pyramid --layout` does not. A bad or non-finite
  `--geo-origin`, `--geo-scale`, `--affine` or `--background` is exit 2 (#68).

### Changed

- A feature this build left out exits 1 everywhere. `--trace-level`,
  `packfile://` and `s3://` used to exit 2 for it while a missing codec exited
  1. The README's "Exit codes" section now holds the whole contract (#64).
- `dECMC` is GOLDEN-ONLY rather than BOUNDED-TOL in `OP_MAP.md`, so its oracle
  class in `viprs __dump-commands --json` changes (#67).
- `viprs pyramid` exits 2, not 1, for a malformed `--geo-origin` or
  `--geo-scale`, and refuses a non-finite one, since it now shares
  `viprs geo`'s parser (#68).

### Fixed

- A FIFO or `<(...)` input decodes. It used to be opened by the SVG sniff,
  which ate its first 4 KB, and then reopened by a decoder that seeks; it is
  now read once and decoded from its bytes (#64).
- A `.svgz`, or a gzipped document named `.svg`, is refused saying the SVG
  renderer in this build has no gzip support, instead of failing as an XML
  parse error (#64).
