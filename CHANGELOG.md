# Changelog

Notable changes to `viprs` land here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
the `version` in `Cargo.toml`.

## [Unreleased]

### Added

- `viprs info --json`, one JSON object per file in the `{"v": 1, ...}` shape
  `features --json` uses, with the exact decoded size (#82).
- `viprs pyramid --format webp`, which writes lossless WebP tiles (#60).
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
- `viprs pyramid` pipeline controls: `--pmtiles-layout tile-id|arrival`,
  `--ordered-emission`, `--dedupe-memory-bytes`, `--checkpoint-every`,
  `--checkpoint-root`, `--retries`, `--retry-backoff-ms`, `--fail-fast`,
  `--skip-failed`, `--checksum`, `--manifest-source-hash`, `--region`,
  `--drop-blanks` and `--events none|text|json`. `--retries N` retries and then
  fails; only `--skip-failed` skips a tile, and a run that skipped any exits 1.
  `--drop-blanks` leaves blank tiles out and can't be combined with
  `--skip-blank`, which writes a placeholder instead. Each `--events json` line
  carries `"v":1` and a fixed event name (#66).
- `viprs verify PATH` checks an archive or a tile tree and names every missing
  or damaged tile, up to 50. `--source FILE` re-renders a tree and compares;
  `--centre` and `--drop-blanks` say what the pyramid was written with, since
  neither the manifest nor the archive records it yet (#66).

### Changed

- The stub store an `s3://` sink writes through under the hidden
  `--object-store-root DIR` is the core's `DirectoryObjectStore` now, with
  the same `DIR/bucket/key` layout and the same tile bytes; the CLI's private
  copy is gone. The core's store is stricter: a bucket name with a backslash
  is refused as a usage mistake (exit 2) like the other bad names, a key
  that crosses a symlink under the root is refused, so the run fails (exit 1)
  rather than write where the link points, and each object is staged through
  a uniquely named `.libviprs-part` file (#87).
- An op command that refuses a value on the command line alone now exits 2,
  the same as clap and the built-ins, with a hint naming the op's `--help`.
  That covers inverted or NaN `clamp` bounds, a `gamma --exponent` out of
  range, vector arguments that don't parse or have the wrong length, a
  negative `extract_area` coordinate, conflicting `thumbnail` flags, an
  unsupported `--crop` or `--mblend`, an output extension nothing writes and
  `-` where a file is needed. A refusal the input decides is still exit 1
  (#78).

- A feature this build left out exits 1 everywhere. `--trace-level`,
  `packfile://` and `s3://` used to exit 2 for it while a missing codec exited
  1. The README's "Exit codes" section now holds the whole contract (#64), and
  says which refusals in the op commands are still a 1 (#79).
- `dECMC` is GOLDEN-ONLY rather than BOUNDED-TOL in `OP_MAP.md`, so its oracle
  class in `viprs __dump-commands --json` changes (#67).
- `viprs pyramid` exits 2, not 1, for a malformed `--geo-origin` or
  `--geo-scale`, and refuses a non-finite one, since it now shares
  `viprs geo`'s parser (#68).
- Every `viprs pyramid` run into an archive, a tile tree or `s3://` goes
  through one driver; only `--packfile` and a fresh `--memory-budget` run
  into a tree keep their own engines. Ctrl-C stops any run, those two
  included, at the next tile with exit 130 instead of dying by the signal,
  and a tile tree it leaves finishes with `--resume` (#66).
- A tile tree run writes its resume checkpoint every 1000 tiles by default
  (#66).
- `--memory-budget` into an archive or `s3://`, or with `--resume` or
  `--verify`, says on stderr that it does not apply. It always ran those on
  the monolithic engine, without saying so (#66).
- `--trace-level` output goes to stderr, so stdout carries only `--events`
  (#66).
- `viprs features --json` prints `{"v":1,"features":[...]}`; the `v` is new
  (#66).
- `s3://` sinks write through a local stub store behind a hidden
  `--object-store-root DIR`, for testing, until the core has a network
  transport. Without the `s3` feature an `s3://` sink exits 1 (#66).

### Fixed

- `pyramid --resume` refuses a checkpoint made from a different input file,
  even one with the same size, with the plan-hash mismatch (exit 1) and a
  hint that the input has to match. Every tile-tree run from a file now
  hands the core the BLAKE3 of the file's bytes, which the core folds into
  the plan hash and records as the manifest's `source.bytes_hash` under
  `--manifest-source-hash`; the CLI no longer patches `manifest.json` after
  the run. A tree run reads its input once more to hash it, and a checkpoint
  left by an earlier `viprs` (which carried no digest) can't be resumed by
  this one (#86).
- `pyramid`, `plan` and `test-image` refuse a zero `--tile-size`, `--dpi`,
  `--page`, width or height, an `--overlap` as wide as the tile, a JPEG
  `--quality` outside 1 to 100, and `plan`'s missing `--height` as usage
  mistakes (exit 2) before reading anything. They used to exit 1 after
  decoding the input, and quality 0 or 101 was taken as given (#81).
- A `--geo-origin`, `--geo-scale` or `--affine` with its value left out no
  longer takes the next flag as the value. `pyramid --geo-origin --centre`
  used to exit 0 having applied neither; it now exits 2 naming `--centre`.
  `pyramid` also refuses one of `--geo-origin` / `--geo-scale` without the
  other, which it used to drop without a word (#75).
- A FIFO or `<(...)` input decodes. It used to be opened by the SVG sniff,
  which ate its first 4 KB, and then reopened by a decoder that seeks; it is
  now read once and decoded from its bytes (#64).
- A `.svgz`, or a gzipped document named `.svg`, is refused saying the SVG
  renderer in this build has no gzip support, instead of failing as an XML
  parse error (#64).
- `svgload` gives that same refusal for a gzipped document, by path or on
  stdin. It used to tell a build without `svg` to rebuild with it, and a
  build with it passed on usvg's "enable svgz cargo feature" parse error
  (#74).
