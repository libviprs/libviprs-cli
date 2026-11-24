# viprs coverage of libviprs and pvec

I keep this file as the map of what `viprs` exposes against what the two libraries it sits on can do. It is the follow-up to EPIC Q (#63), which brought the CLI level with libviprs 0.5.0. Since then libviprs grew DWG and PDF to PVec conversion and the CLI started reading `.pvec` files through PVec's own facade, so I re-audited both libraries against `viprs` at `b32a5b3` (2026-11-23), and again for CV2.1 (#150) against libviprs `4b443f2f` (2026-11-24), the pin this file now describes. The epic that closes the gaps is #147.

Status words:

- **covered**: a command or flag reaches it and a test drives it.
- **partial**: reachable, but some public options of it are not.
- **missing**: public in the library, no way to reach it from `viprs`. The task id says which epic task closes it.
- **not-CLI**: deliberately left out, with the reason.

## Pins

| Dependency | Pinned rev | Upstream main | What the difference holds |
|---|---|---|---|
| libviprs | `4b443f2f` (moved from `71cbbe0` by CV2.1) | `4b443f2f`, the pin is main's tip | The old pin was 312 commits behind this main, not the 44 the first audit counted. Under `src/` the delta was 37 files, +9704/-705, all in `src/pvec`, `src/pvec_convert`, `src/cad`, `src/cancel.rs`, `src/observe.rs` and `src/lib.rs`: `paper_sheets`, `PvecConversion::{annotations, element_rules, thumbnail, plot_settings, deadline}`, `pvec::thumbnail`, the plot side (`PdfPlot`, PC3 and plot style files, `CadManifest` and layer choices), PDF layers and xrefs. Ops, codecs, engine, sinks and PMTiles are identical, and the crate is still 0.6.1. |
| pvec (facade, capabilities, container, components, geometry) | `e1a768ba` | `419535c` (12 commits ahead) | `PreviewStyle` (viewer only) and four new paint-driver capabilities. Nothing a CLI needs. libviprs `4b443f2f` pins the same `e1a768ba`, and `tests/pvec_rev_lockstep_150.rs` fails if this crate and the pinned core ever name different ones. |

The pin is its own commit (the first of CV2.1), gated on its own, so a regression bisects to the pin and not to a command. The pvec pin stays with the core's; when the core moves it, the same test says so.

## Cargo features

| libviprs feature | viprs feature | Status |
|---|---|---|
| `dwg-acadrust` (core default) | `dwg-acadrust` (default) | covered |
| `dwg-acadsharp` | `dwg-acadsharp` | covered |
| `pdfium`, `pdfium-static` | same names | covered |
| `svg`, `jxl`, `jp2k`, `avif` | same names | covered |
| `packfile`, `tracing` | same names | covered |
| `object-store-sink`, `s3` | same names | covered (see the object store row) |
| `pvec-raster`, `pvec-convert` | same names (default) | covered |
| `pvec-write`, `pvec-normalized`, `pvec-spline` | through `pvec-convert` and `dwg-acadrust` | covered |
| `pvec-raster-thumbnail` | `pvec-raster-thumbnail` (in `full`, not in `default`) | covered (CV2.1) |
| `jbig2` | none | missing, CV2.2 (JBIG2 image streams in a PDF page) |
| `serde` | none | not-CLI: it only widens the Rust API; every viprs `--json` output is its own schema |
| `test-util` | none | not-CLI: a sink that sleeps on purpose, for stress tests |

## Image ops (vips families)

| Area | Status |
|---|---|
| 171 op commands across arithmetic, composite, resample, colour, draw, bands, conversion, convolution, morphology, histogram, mosaicing, freqfilt, create, extract, matrix and core | covered: every non-excluded `OP_MAP.md` row is registered, including the `8afc3ae5` re-audit residue (`remainder`, `sobel`, `scharr`, `prewitt`, `canny`, `matrixmultiply`, `join`) |
| 26 `EXCLUDED` rows | not-CLI, each with its reason in `OP_MAP.md` (builders, accessors, `downscale_*` helpers) |

## Codecs

| Capability | viprs | Status |
|---|---|---|
| JPEG save (`Q`, subsample) and load (`shrink`) | `jpegsave`, `jpegload` | covered |
| JPEG restart interval | none | not-CLI: `jpegsave_buffer_restart` is a refusal stub in the core |
| PNG save: deflate level, interlace, palette | `pngsave` | covered |
| PNG bilevel save (`encode_png_bilevel`) | `pngsave --bitdepth 1` on a one-band image, as vips does | covered (CV1.5) |
| TIFF save: none, LZW, deflate; load: page, max-pages | `tiffsave`, `tiffload` | covered |
| TIFF JPEG, CCITT and JPEG 2000 compression, tiled, pyramid, sub-IFD, BigTIFF | none | not-CLI: refusal stubs in the core (`save_tiff_tiled`, `save_bigtiff`, `unsupported_compression`) |
| WebP save (lossless) and load (page, n) | `webpsave`, `webpload` | covered |
| WebP save `keep` (metadata chunks) | `webpsave --keep` (`none`, the default, or `all`) | covered (CV2.3) |
| GIF save (dither, bitdepth, interlace) and load (page, n, max-pages) | `gifsave`, `gifload` | covered |
| JPEG XL save (lossless) | `jxlsave` | covered |
| JPEG XL load `page`, `n` (`decode_jxl_with`) | `jxlload --page --n` | covered (CV1.5) |
| JPEG 2000 save lossless with tile size; load | `jp2ksave`, `jp2kload` | covered |
| JPEG 2000 lossy (`Compression::Lossy { ratio }`) | `jp2ksave --ratio N`, a hidden viprs extension because vips takes a `Q` | covered (CV1.5) |
| Radiance save `exposure`, `aspect` | `radsave --exposure --aspect` (hidden viprs extensions) | covered (CV1.5) |
| Radiance, FITS, CSV, matrix, PPM load and save; Ultra HDR load and save (`Q`, gain map shrink) | the vips-named loaders and savers | covered |
| OpenEXR, NIfTI, Analyze, Matlab, AVIF, SVG (dpi, scale, unlimited) load | the vips-named loaders | covered |
| `.v` read and write | every op OUT, `copy` | covered |
| Metadata fields (`get_fields`, `get_field`) | `viprs info --fields`, the `vipsheader -a` view | covered (CV1.5) |
| Setting arbitrary metadata fields | none | not-CLI: vips has no CLI for it either; `copy` covers the typed ones |
| HEIF/HEIC save, OpenSlide, ImageMagick, `dzsave_buffer` | none | not-CLI: `foreign_stubs`, refusal stubs |
| `decode_file_sequential` | none | not-CLI: a documented alias of `decode_file` |
| Load cache knobs | none | not-CLI: one process per command, there is nothing to reuse |
| Header probe (`probe_file`, `probe_bytes`) | `info`, the pyramid pre-flight | covered |

## PDF

| Capability | viprs | Status |
|---|---|---|
| `pdf_info` (with password), `page_rotate` | `info`, `pdf info`, `pdf rotation` | covered |
| Embedded image extract (dpi, sizing, background, password) | `pdf extract`, `pyramid` | covered |
| PDFium render (budgeted) | `pyramid --render`, `pdf extract --dpi --render-budget` | covered |
| `PdfiumStripSource` streaming render and `BudgetPolicy::AutoAdjustDpi` | `pyramid --render --render-stream`, `--min-dpi`, into a tile tree under `--memory-budget` | covered (CV2.3) |

## Pyramid engine

| Capability | viprs `pyramid` | Status |
|---|---|---|
| Planner: tile size, overlap, centre, deep-zoom/xyz/google | flags | covered |
| Zoomify and IIIF layouts, `with_iiif_id` | `plan`, and `pyramid --layout zoomify` or `iiif` with `--iiif-id` into a directory or an object store | covered (CV2.3) |
| Plan queries (peak memory, DZI manifest, sidecars, tile path, tile rect) | `plan` | covered |
| Concurrency, buffer size, memory budget, parallel, region | flags | covered |
| Explicit `EngineKind` (monolithic, streaming, MapReduce, MapReduce hot cache) | `--engine` | covered (CV2.3) |
| Background colour (`with_background_rgb`) | `--background R,G,B` | covered (CV2.3) |
| Blank tiles (emit, placeholder, tolerance, drop) | flags | covered |
| Failure policy and retry (count, initial backoff) | `--on-failure`, `--retries` | covered |
| Retry multiplier and jitter | `--retry-multiplier`, `--no-retry-jitter` | covered (CV2.3) |
| Resume, overwrite, verify, checkpoint every/root | flags | covered |
| Dedupe (blanks, all), checksums (emit, verify, blake3, sha256), manifest source hash | flags | covered |
| Observers and events | `--events text|json` | covered |
| Cancel token | SIGINT | covered |
| `raster_verify`, `pyramid_verify`, `verify_from_strip_source`, `verify_output` | `verify` | covered |
| Custom `WorkExecutor`, `Extensions`, `RetryingSink`, `MemorySink` | none | not-CLI: in-process composition API |

## Sinks and storage

| Capability | viprs | Status |
|---|---|---|
| Filesystem tree (manifest, checksums, dedupe, resume) | `--storage directory` | covered |
| PMTiles sink (layout, ordered emission, dedupe window, geo transform) | `--storage pmtiles` | covered |
| PMTiles writer metadata (name, description, attribution), internal compression, bounds, centre | `--pmtiles-name`, `--pmtiles-description`, `--pmtiles-attribution`, `--pmtiles-internal-compression`, `--pmtiles-bounds`, `--pmtiles-centre` | covered (CV2.3) |
| PMTiles tile compression | set from `--format` | not-CLI: the field describes the stored blobs, and PNG, JPEG and WebP tiles are compressed already, so a flag could only mislabel them |
| Packfile (tar, tar.gz, zip) | `--packfile`, `packfile://` | covered |
| Object store sink (key prefix, image name) | the object-store key layout written into a directory; `object-store://`, with `s3://` as a deprecated alias | covered (CV2.3) |
| Network object store (S3, HTTP) | none | not-CLI: the core ships no network transport by design (libviprs#1119); upload the directory with `aws s3 sync` or `rclone` |
| PVec raster sink | `--storage pvec` | covered |

## PMTiles

| Capability | viprs `pmtiles` | Status |
|---|---|---|
| Reader (header, metadata, tiles, bounds, geo transform) | `info`, `tile` | covered |
| Validation | `verify` (its own walk, stricter than go-pmtiles) | covered |
| Directory to archive (`migrate_to_pmtiles`) | `pack` | covered |
| Archive to tree | `extract` | covered |
| Archive into a `.pvec` | `to-pvec` | covered |
| `ObjectStoreRangeReader` | none | not-CLI: the core ships no network object store (libviprs#1119) |

## Geo

`GeoTransform` pixel-to-geo, geo-to-pixel and tile centre: covered by `viprs geo`.

## CAD and vector tiles

| Capability | viprs `vector` | Status |
|---|---|---|
| `list_views`, `convert`, `report`, `inspect`, `check` | `views`, `build`, `report`, `info`, `validate` | covered |
| Provider choice and hardened limits | `--provider`, `--hardened` | covered |
| `TilerConfig` knobs past max zoom and deadline (extent, buffer, margin, deviation, simplify, cull, min text, per-tile and per-level caps, max primitives, max tile bytes, oversize policy, max text bytes) | `build`, `report`: one flag each (`--extent` ... `--oversize`) | covered (CV2.4) |
| `AcadrustDecoder::with_limits` (`SourceLimits`), `AcadSharpDecoder::with_limits` (`ProviderLimits`) | `views`, `build`, `report`: one flag per limit, layered over `--hardened` | covered (CV2.4) |
| `display_text` (MTEXT escapes) | none | not-CLI: a helper the tiler already applies |

## PVec inside libviprs

| Capability | viprs `pvec` | Status |
|---|---|---|
| DWG to `.pvec` per paper layout, one layout, or model space; stem; unit; geometry only; regions | `convert` | covered |
| `PvecConversion::annotations` (TEXT, MTEXT and the rest into the text stream, HATCH and SOLID into the hatch stream) | `convert --annotations` | covered (CV2.1); `texts` and `fills` on their own are missing, later |
| `PvecConversion::element_rules` (layer, block, colour and lineweight rules for the regions step) | `convert --regions --element-rule` | covered (CV2.1) |
| `PvecConversion::thumbnail`, `pvec::thumbnail` (a PNG of a sheet, strokes only) | `convert --thumbnail`, `pvec thumbnail` (feature `pvec-raster-thumbnail`) | covered (CV2.1) |
| `recalibrate` (set, replace or remove the scale record) | `pvec calibrate`, through the facade's `recalibrate_file` | covered (CV1.4) |
| `PdfConversion` (a PDF page to `.pvec`, image limits) | none | missing, CV2.2 |
| `ConversionProfile`, `CapabilityPolicy`, `allowed_capabilities` | none | missing, CV2.2 |
| Calibration at convert time (`Calibration`, `ScaleClaim`, georeference sidecar import) | none | missing, CV2.2 |
| `ViewProfile` knobs, `SourceLimits`, `LeafConfig`, `SpillConfig`, `RegionConfig` tuning | none | missing, CV2.2 |
| `paper_sheets` (list paper layouts without converting) | `pvec layouts` | covered (CV2.1) |
| `PdfPlot` (DWG layouts to a PDF), `PlotSettings`, PC3 and plot style files (`load_pc3`, `load_plot_style`) | none | missing, #157 (`viprs plot`) |
| `cad_manifest`, `CadChoices`, `apply_choices`, `check_layer_state`, `resolve_visibility` (layer states and visibility choices, `LayerStateNotFound`) | none | missing, later (the plot work in #157 reads it first) |
| `PdfConversion` layer choices (`pdf_manifest`, `apply_pdf_layers`, `PdfLayerChoices`) | none | missing, CV2.2 |
| `PvecConversion::{images, xref, plot_settings, plot_style_table}` (DWG images, external references, plot settings at convert time) | none | missing, later |
| `deadline`, `cancel` and `observer` on `PvecConversion`, `PvecLayoutSet` and `PdfPlot`; the `code()` strings of the error types | SIGINT is the cancel token of `convert`; no deadline flag, no observer, the codes are not printed | partial, later |
| Raster pyramids into `.pvec` | `pyramid --storage pvec`, `pmtiles to-pvec`, `pvec rasters` | covered |
| `drift`, `dump` | none | not-CLI: fixture regression tooling (the `pvec_drift` example) |

## PVec facade and crates

| Capability | viprs | Status |
|---|---|---|
| `PvecReader::manifest`, `entity_count`, the stream list (name, version, size) and skipped streams | `pvec info` | covered (CV1.1) |
| Layer and text streams decoded | `pvec info` (layer names, text count), `pvec text`, `pvec validate` | covered (CV1.1, CV1.2, CV1.3) |
| Line style, hatch and provenance streams decoded | `pvec validate` decodes them, no test writes one | partial |
| Paint order, stroke and fill paint, clip, semantic class streams | `pvec info` lists them by name; nothing decodes them | partial |
| `PvecReader::{calibration, georeference, effective_scale}` | `pvec info` (scale with its provenance, the georeference object) | covered (CV1.1) |
| PVec's validation: container structure and limits, block checksums, artifact digest (`ContainerReader::{validate, verify_artifact_digest}`) | `pvec validate`, held to PVec's own validator in the tests | covered (CV1.2) |
| Each raster's tiles (`validate_raster`) and the CAL-11 residual (`check_residual`), reported without failing the file | `pvec validate` | covered (CV1.2) |
| `TextStream` records | `pvec text` | covered (CV1.3) |
| `recalibrate_file` | `pvec calibrate` | covered (CV1.4) |
| Raster placements and tiles | `pvec rasters` | covered |
| `PickIndex` (point, rect, nearest) | none | missing, CV2.5 |
| `Tessellate` against an error budget | none | missing, CV2.5 |
| `pvec-interchange` exports (GML, GeoPackage, MVT, STEP, fidelity report) | none | missing, CV2.5 (a new direct dependency at the core's pvec rev) |
| Streaming API (spatial directory, Tier 0, viewport queries) | none | not-CLI: incremental loading for a viewer; a one-shot command has no viewport. `pvec info` reports whether the directory exists |
| `pvec-scene` paint plan | none | not-CLI: PVec ships no rasteriser, so there is no `.pvec` to image path to expose (owner question) |
| `pvec-wasm` | none | not-CLI: the browser ABI |
| PVec's own `pvec` binary: `inspect`, `validate`, `calibrate`, `dump-manifest`, `capabilities` | `pvec info`, `pvec validate`, `pvec calibrate`, `pvec info --json`, `pvec info` (manifest capabilities and skipped streams) | covered (CV1.1, CV1.2, CV1.4) |
| PVec's own `pvec tessellate` | none | missing, CV2.5 |
| `pvec compare-step` | none | not in this epic: it needs an external OpenCASCADE evaluator report |
| `pvec bench-open` | none | not-CLI: a benchmark, which only runs through the Mercury queue |

## Examples and binaries

libviprs has no binaries. Its five examples (`dwg_gate0`, `pvec_components_cost`, `pvec_drift`, `pvec_writer_memory`, `text_policy_measure`) are measurement and fixture tools, so they are not-CLI. PVec's only user-facing binary is the `pvec` CLI in `pvec-conformance`, mapped above.
