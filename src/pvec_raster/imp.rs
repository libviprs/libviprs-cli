//! What the `.pvec` raster commands do once the command line is settled (libviprs-cli#130). Needs
//! the `pvec-raster` feature: this is the only file here that names PVec's facade.
//!
//! The `@doc-snippet` slots are for the libviprs.org CLI page. The site's extractor reads
//! `main.rs` and `pipeline.rs` today, so it has to be taught this file before its next re-sync.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use libviprs::raster_pvec::placement::{SourceCalibration, pixel_to_source};
use libviprs::raster_pvec::pyramid::{
    RasterBand, RasterChannels, RasterPlacement, RasterPyramidOptions, prepare_raster,
};
use libviprs::raster_pvec::tiles::tile_tags;
use libviprs::sink_pvec::PvecSink;
use libviprs::{
    CancelToken, EngineBuilder, EngineKind, Layout, PixelOrigin, PyramidPlanner, TileFormat,
};
use pvec_facade::components::LayerStream;
use pvec_facade::unstable::{Band, PixelFormat, Placement, RasterId, TileCodec, TileIn};
use pvec_facade::{PvecReader, PvecWriter};

use super::{Checked, CheckedToPvec, Looks, Spec, TileAt};
use crate::{PyramidArgs, operational_error, pipeline};

/// The CRS both ends of a composed placement are given. The command line carries no CRS (ADR 0018
/// open question 2), so a caller asserts that the raster and the calibration share one, and the
/// composition's CRS check compares this with itself.
const ASSERTED_CRS: &str = "asserted-by-the-caller";

/// `err` and every `source()` under it, joined: the facade and the core keep the reason in the
/// chain and the top message names only the step.
fn chain(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut source = err.source();
    while let Some(next) = source {
        text.push_str(": ");
        text.push_str(&next.to_string());
        source = next.source();
    }
    text
}

fn read_file(path: &Path) -> Vec<u8> {
    std::fs::read(path)
        .unwrap_or_else(|e| operational_error(&format!("reading {}: {e}", path.display())))
}

fn open_pvec(path: &Path) -> PvecReader {
    PvecReader::open(read_file(path)).unwrap_or_else(|e| {
        operational_error(&format!(
            "{} is not a .pvec this build reads: {}",
            path.display(),
            chain(&e)
        ))
    })
}

/// A writer holding the drawing's entities, ready to take a raster, and a layer table when asked.
///
/// The facade reads entities and writes them back as they were stored ("writing what is read back
/// writes the same bytes"), but it cannot hand over component streams or styles. So a drawing that
/// declares any capability, or styles an entity, is refused rather than copied with those lost.
// @doc-snippet:begin slot=pvec-join imports=PvecReader,PvecWriter
fn open_join(drawing: &Path, layer: Option<&str>) -> PvecWriter {
    let reader = open_pvec(drawing);
    let manifest = reader.manifest().clone();
    if !manifest.capabilities.is_empty() {
        operational_error(&format!(
            "{} declares capabilities ({:?}) that adding a raster cannot carry over yet; only a \
             drawing of plain curves can be joined",
            drawing.display(),
            manifest.capabilities
        ));
    }
    let entities = reader.entities().unwrap_or_else(|e| {
        operational_error(&format!(
            "reading the entities of {}: {}",
            drawing.display(),
            chain(&e)
        ))
    });
    let styles = reader.styles().unwrap_or_else(|e| {
        operational_error(&format!("reading {}: {}", drawing.display(), chain(&e)))
    });
    if styles.iter().any(|(_, style)| *style != 0) {
        operational_error(&format!(
            "{} has styled entities, which adding a raster cannot carry over yet",
            drawing.display()
        ));
    }
    let mut writer = PvecWriter::new(manifest)
        .unwrap_or_else(|e| operational_error(&format!("starting the output: {}", chain(&e))));
    for (id, curve) in &entities {
        writer.push(*id, curve).unwrap_or_else(|e| {
            operational_error(&format!(
                "copying an entity of {}: {}",
                drawing.display(),
                chain(&e)
            ))
        });
    }
    if let Some(name) = layer {
        let table = LayerStream {
            names: vec![name.to_string()],
            flags: vec![0],
            assignments: Vec::new(),
        };
        let stream = table
            .into_stream(&pvec_capabilities::Registry::v1())
            .unwrap_or_else(|e| operational_error(&format!("--layer {name:?}: {}", chain(&e))));
        writer.components(stream).unwrap_or_else(|e| {
            operational_error(&format!("writing the layer table: {}", chain(&e)))
        });
    }
    writer
}
// @doc-snippet:end slot=pvec-join

/// `viprs pyramid --storage pvec`.
pub(super) fn pyramid(args: PyramidArgs, checked: Checked, cancel: CancelToken) {
    let start = Instant::now();
    let Checked {
        into,
        output,
        pixel_to_geo,
        spec,
        looks,
    } = checked;
    let Looks {
        band,
        opacity,
        attribution,
        layer,
    } = looks;

    let writer = open_join(&into, layer.as_deref());
    let tile_format = match args.format {
        crate::FormatArg::Png => TileFormat::Png,
        crate::FormatArg::Jpeg => TileFormat::Jpeg {
            quality: args.quality,
        },
        crate::FormatArg::Webp => TileFormat::Webp,
        crate::FormatArg::Raw => unreachable!("--format raw is a usage error before this point"),
    };
    let pipeline::Prepared { raster, plan } = pipeline::prepare(&args, Layout::Google);

    let placement = match (spec, pixel_to_geo) {
        (Spec::Explicit(m), _) => RasterPlacement::Explicit(m),
        (Spec::Composed { source_to_geo }, Some(pixel_to_native)) => RasterPlacement::Composed {
            pixel_to_native,
            // `--geo-origin` is the top-left corner of the first pixel, and so is `--affine`'s.
            pixel_origin: PixelOrigin::Corner,
            raster_crs: ASSERTED_CRS.to_string(),
            calibration: SourceCalibration::Affine {
                native_crs: ASSERTED_CRS.to_string(),
                // The calibration is given in source units, so there is no unit scale to fold in.
                metres_to_native: source_to_geo,
                metres_per_unit: 1.0,
            },
        },
        (Spec::Composed { .. }, None) => unreachable!("a composed placement has a transform"),
    };
    let mut options = RasterPyramidOptions::new(placement);
    options.tile_format = tile_format;
    options.skip_blanks = args.pipeline.drop_blanks;
    options.attribution = attribution;
    options.opacity = opacity;
    options.layer_index = layer.as_ref().map(|_| 0);

    // @doc-snippet:begin slot=pvec-sink imports=prepare_raster,PvecSink,EngineBuilder
    let mut job =
        prepare_raster(&raster, &plan, &options).unwrap_or_else(|e| operational_error(&chain(&e)));
    job.pyramid.band = if band == 1 {
        RasterBand::Overlay
    } else {
        RasterBand::Underlay
    };
    for note in job
        .pyramid
        .report
        .conversions
        .iter()
        .chain(&job.pyramid.report.forced)
    {
        eprintln!("Note: {note}");
    }
    let sink = PvecSink::new(writer, &job.pyramid, &options)
        .unwrap_or_else(|e| operational_error(&chain(&e)));

    let events = pipeline::EventPrinter::new(args.pipeline.events);
    let mut builder = EngineBuilder::new(job.source.as_ref(), job.pyramid.plan.clone(), &sink)
        .with_config(pipeline::engine_config(&args))
        .with_cancel(cancel)
        .with_observer_arc(events.clone());
    if let Some(budget_mb) = args.memory_budget {
        // The sink takes tiles from any engine, so `--memory-budget` bounds the pixel side
        // the way it does for a tree; the encoded pyramid is PVec's writer's to hold.
        let bytes = if budget_mb == 0 {
            job.pyramid
                .plan
                .estimate_peak_memory_for_format(raster.format())
                / 4
        } else {
            crate::mb_to_bytes(budget_mb)
        };
        builder = builder
            .with_engine(if args.parallel {
                EngineKind::MapReduce
            } else {
                EngineKind::Streaming
            })
            .with_memory_budget(bytes)
            .with_budget_policy(libviprs::BudgetPolicy::Error);
    }
    let result = builder.run();
    // @doc-snippet:end slot=pvec-sink
    pipeline::conclude(result, &output, start, pipeline::Resumable::No, &events);

    let (stored, padding) = (sink.stored_tiles(), sink.dropped_padding_tiles());
    let staging = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    sink.finish_to(&output, &staging).unwrap_or_else(|e| {
        operational_error(&format!("writing {}: {}", output.display(), chain(&e)))
    });
    eprintln!(
        "Wrote {} into {}: {stored} tiles stored, {padding} past the grid left out, {} levels, \
         tile edge {}",
        output.display(),
        into.display(),
        job.pyramid.levels,
        job.pyramid.tile_edge,
    );
}

/// `viprs pmtiles to-pvec`: the archive's tiles go in as they are stored.
// @doc-snippet:begin slot=pvec-passthrough imports=Reader,tile_tags,pixel_to_source
pub(super) fn to_pvec(archive: &Path, output: &Path, checked: CheckedToPvec) {
    use libviprs::pmtiles::{Compression, TileType};

    let CheckedToPvec {
        into,
        explicit,
        source_to_geo,
        looks,
    } = checked;
    let reader = crate::open_archive(archive);
    let header = reader.header();
    let refuse = |why: String| -> ! {
        operational_error(&format!(
            "{} cannot be passed through to a .pvec: {why}",
            archive.display()
        ))
    };

    let codec = match header.tile_type {
        TileType::Png => TileCodec::Png,
        TileType::Jpeg => TileCodec::Jpeg,
        TileType::Webp => TileCodec::Webp,
        other => refuse(format!(
            "its tiles are {}, and a .pvec stores PNG, JPEG or WebP",
            crate::tile_type_name(other)
        )),
    };
    let meta = reader
        .metadata()
        .unwrap_or_else(|e| refuse(format!("its metadata did not parse: {e}")));
    let (Some(vnd), attribution) = (meta.vnd_libviprs.as_ref(), meta.attribution.clone()) else {
        refuse(
            "it carries no `vnd.libviprs` metadata, so its grid cannot be checked; make it with \
             `viprs pyramid --layout google`"
                .to_string(),
        )
    };
    let (Some(generation), Some(source)) = (vnd.generation.as_ref(), vnd.source.as_ref()) else {
        refuse(
            "it was assembled from tiles, so its source size and grid are not recorded; make it \
             with `viprs pyramid --layout google`"
                .to_string(),
        )
    };
    if generation.centre {
        refuse(
            "it was generated centred, and a raster in a .pvec is anchored at pixel (0,0)".into(),
        );
    }
    if generation.overlap != 0 {
        refuse(format!(
            "it was generated with an overlap of {}, and tiles of a .pvec do not overlap",
            generation.overlap
        ));
    }
    if generation.layout != Layout::Google {
        refuse(format!(
            "it was generated as {:?}, and a .pvec stores the Google grid (full square tiles); \
             run `viprs pyramid --layout google`",
            generation.layout
        ));
    }
    let edge = generation.tile_size;
    if !super::TILE_EDGES.contains(&edge) {
        refuse(format!("its tile size {edge} is not 256, 512 or 1024"));
    }
    let (width, height) = (source.width, source.height);
    let plan = PyramidPlanner::new(width, height, edge, 0, Layout::Google)
        .unwrap_or_else(|e| refuse(format!("its source size {width}x{height}: {e}")))
        .plan();
    let levels = plan.levels.len();
    if header.min_zoom != 0 || usize::from(header.max_zoom) + 1 != levels {
        refuse(format!(
            "its zooms {}-{} are not the 0-{} of a {width}x{height} Google pyramid",
            header.min_zoom,
            header.max_zoom,
            levels - 1
        ));
    }

    let pixel_to_source_map = match (explicit, reader.geo_transform()) {
        (Some(direct), _) => direct,
        (None, Ok(Some(stored))) => {
            let calibration = source_to_geo
                .expect("a command line with no --pixel-to-source has --source-to-geo");
            pixel_to_source(
                &stored.transform(),
                stored.pixel_origin,
                ASSERTED_CRS,
                &SourceCalibration::Affine {
                    native_crs: ASSERTED_CRS.to_string(),
                    metres_to_native: calibration,
                    metres_per_unit: 1.0,
                },
            )
            .unwrap_or_else(|e| {
                refuse(format!(
                    "the placement could not be composed: {}",
                    chain(&e)
                ))
            })
        }
        (None, Ok(None)) => refuse(
            "it stores no georeference (it was made without --geo-origin, --geo-scale or \
             --affine); give the placement with --pixel-to-source"
                .to_string(),
        ),
        (None, Err(e)) => refuse(format!("its stored georeference did not parse: {e}")),
    };

    let (codec_row, channels) = tile_tags(generation.format, source.pixel_format, 8)
        .unwrap_or_else(|e| {
            refuse(format!(
                "its tiles were cut from a {:?} source ({e}); `viprs pyramid --storage pvec` \
                 converts before tiling, which a passthrough cannot",
                source.pixel_format
            ))
        });
    let _ = codec_row;
    let pixel_format = match channels {
        RasterChannels::Gray => PixelFormat::Gray,
        RasterChannels::Rgb => PixelFormat::Rgb,
        _ => PixelFormat::Rgba,
    };

    let mut tiles = Vec::new();
    let mut found = 0u64;
    for (level, l) in plan.levels.iter().enumerate() {
        let level = u32::try_from(level).expect("a pyramid has at most 32 levels");
        let (cols, rows) = (l.width.div_ceil(edge), l.height.div_ceil(edge));
        for row in 0..rows {
            for col in 0..cols {
                let z = u8::try_from(level).expect("a pyramid has at most 32 levels");
                let stored = reader
                    .get_tile(z, col, row)
                    .unwrap_or_else(|e| refuse(format!("tile {level}/{col}/{row}: {e}")));
                let Some(stored) = stored else { continue };
                found += 1;
                // The archive's own tile compression is the container's, not the image's: the
                // image bytes inside are what a .pvec keeps, and they are not re-encoded.
                let bytes = if matches!(header.tile_compression, Compression::None) {
                    stored
                } else {
                    header
                        .tile_compression
                        .decompress(&stored, crate::MAX_TILE_DECOMPRESSED)
                        .unwrap_or_else(|e| refuse(format!("tile {level}/{col}/{row}: {e}")))
                };
                tiles.push(TileIn {
                    level,
                    col,
                    row,
                    bytes,
                });
            }
        }
    }
    let past_the_grid = header.addressed_tiles_count.saturating_sub(found);

    let placement = Placement {
        pixel_to_source: pixel_to_source_map,
        width,
        height,
        visible_box: [0, 0, width, height],
        tile_edge: u16::try_from(edge).expect("an edge of 256, 512 or 1024 fits u16"),
        levels: u8::try_from(levels).expect("a pyramid has at most 32 levels"),
        tile_codec: codec,
        pixel_format,
        bit_depth: 8,
        opacity: looks.opacity,
        band: if looks.band == 1 {
            Band::Overlay
        } else {
            Band::Underlay
        },
        layer_index: looks.layer.as_ref().map(|_| 0),
        // The archive records a BLAKE3 of the source file, and this field is a SHA-256.
        source_digest: None,
        attribution: attribution.unwrap_or_default(),
    };
    let mut writer = open_join(&into, looks.layer.as_deref());
    writer.rasters(&[placement]).unwrap_or_else(|e| {
        refuse(format!(
            "PVec's writer refused the placement: {}",
            chain(&e)
        ))
    });
    let id = RasterId::new(0).expect("0 is a valid RasterId");
    let stored_tiles = tiles.len();
    writer
        .raster_tiles(id, tiles)
        .unwrap_or_else(|e| refuse(format!("PVec's writer refused the tiles: {}", chain(&e))));
    let bytes = writer
        .finish()
        .unwrap_or_else(|e| refuse(format!("PVec's writer refused the artifact: {}", chain(&e))));
    publish(&bytes, output);
    eprintln!(
        "Wrote {} into {}: {stored_tiles} tiles passed through byte for byte, {past_the_grid} \
         past the grid left out, {levels} levels, tile edge {edge}",
        output.display(),
        into.display(),
    );
}
// @doc-snippet:end slot=pvec-passthrough

/// Write `bytes` to `dest` so that nothing appears there until all of it has: a staging file
/// beside it, synced, then renamed. The staging file does not outlive a failure.
fn publish(bytes: &[u8], dest: &Path) {
    let staging = dest.with_extension(format!("pvec-staging-{}", std::process::id()));
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&staging, dest)
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&staging);
        operational_error(&format!("writing {}: {e}", dest.display()));
    }
}

fn codec_name(codec: TileCodec) -> String {
    match codec {
        TileCodec::Png => "png".into(),
        TileCodec::Jpeg => "jpeg".into(),
        TileCodec::Webp => "webp".into(),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn pixel_format_name(format: PixelFormat) -> String {
    match format {
        PixelFormat::Gray => "gray".into(),
        PixelFormat::GrayAlpha => "gray_alpha".into(),
        PixelFormat::Rgb => "rgb".into(),
        PixelFormat::Rgba => "rgba".into(),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn band_name(band: Band) -> &'static str {
    match band {
        Band::Overlay => "overlay",
        _ => "underlay",
    }
}

/// The first `RASTER-NN`, `LIMIT-NN` or `INTEGRITY-NN` named in `text`.
fn rule_in(text: &str) -> Option<String> {
    ["RASTER-", "LIMIT-", "INTEGRITY-"]
        .iter()
        .find_map(|prefix| {
            let at = text.find(prefix)?;
            let digits: String = text[at + prefix.len()..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            (!digits.is_empty()).then(|| format!("{prefix}{digits}"))
        })
}

/// `viprs pvec rasters`.
// @doc-snippet:begin slot=pvec-rasters imports=PvecReader
pub(super) fn rasters(file: &Path, json: bool, tile: Option<TileAt>) {
    let reader = open_pvec(file);
    if let Some(at) = tile {
        return write_tile(&reader, file, &at);
    }
    let placements = reader.raster_placements().unwrap_or_else(|e| {
        operational_error(&format!(
            "the raster stream of {} is refused: {}",
            file.display(),
            chain(&e)
        ))
    });

    let mut listed = Vec::new();
    let mut skipped = Vec::new();
    for (id, p) in placements.iter().enumerate() {
        let id = u32::try_from(id).expect("a PVec has at most 4096 rasters");
        let refusal = if let Some(skip) = p.skip() {
            Some(("RASTER-04".to_string(), format!("{skip:?}")))
        } else {
            // The facade at this revision has no call that loads only the index block, so the one
            // tile every pyramid has (level 0 is a single tile) stands in for it: a raster whose
            // index or tile the sniffers refuse fails here, with the rule, and the file does not.
            RasterId::new(id).ok().and_then(|raster| {
                reader.raster_tile(raster, 0, 0, 0).err().map(|e| {
                    let reason = chain(&e);
                    (rule_in(&reason).unwrap_or_default(), reason)
                })
            })
        };
        if let Some((rule, reason)) = refusal {
            skipped.push((id, rule, reason));
            continue;
        }
        listed.push((id, p));
    }

    if json {
        let rasters: Vec<_> = listed
            .iter()
            .map(|(id, p)| {
                serde_json::json!({
                    "id": id,
                    "width": p.width,
                    "height": p.height,
                    "tile_edge": p.tile_edge,
                    "levels": p.levels,
                    "codec": codec_name(p.tile_codec),
                    "pixel_format": pixel_format_name(p.pixel_format),
                    "bit_depth": p.bit_depth,
                    "band": band_name(p.band),
                    "opacity": p.opacity,
                    "layer": p.layer_index,
                    "attribution": p.attribution,
                    "pixel_to_source": p.pixel_to_source,
                    "visible_box": p.visible_box,
                })
            })
            .collect();
        let skipped: Vec<_> = skipped
            .iter()
            .map(|(id, rule, reason)| serde_json::json!({ "id": id, "rule": rule, "reason": reason }))
            .collect();
        println!(
            "{}",
            serde_json::json!({ "v": 1, "rasters": rasters, "skipped": skipped })
        );
        return;
    }

    println!("File: {}", file.display());
    println!("Rasters: {}", listed.len());
    for (id, p) in &listed {
        println!(
            "  {id}: {}x{} px, tile edge {}, {} levels, {} {} {}-bit, band {}, opacity {}, layer {}, attribution {:?}",
            p.width,
            p.height,
            p.tile_edge,
            p.levels,
            codec_name(p.tile_codec),
            pixel_format_name(p.pixel_format),
            p.bit_depth,
            band_name(p.band),
            p.opacity,
            p.layer_index
                .map_or_else(|| "none".to_string(), |l| l.to_string()),
            p.attribution,
        );
    }
    for (id, rule, reason) in &skipped {
        println!("  {id}: skipped, {rule}: {reason}");
    }
}
// @doc-snippet:end slot=pvec-rasters

/// `viprs pvec rasters FILE --tile ID LEVEL COL ROW OUT`: the tile's bytes and nothing else.
fn write_tile(reader: &PvecReader, file: &Path, at: &TileAt) {
    let raster = RasterId::new(at.raster)
        .unwrap_or_else(|e| operational_error(&format!("raster {}: {e}", at.raster)));
    let bytes = match reader.raster_tile(raster, at.level, at.col, at.row) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => operational_error(&format!(
            "raster {} tile {}/{}/{} is not stored in {}",
            at.raster,
            at.level,
            at.col,
            at.row,
            file.display()
        )),
        Err(e) => operational_error(&format!(
            "reading raster {} tile {}/{}/{} from {}: {}",
            at.raster,
            at.level,
            at.col,
            at.row,
            file.display(),
            chain(&e)
        )),
    };
    if at.out == "-" {
        let mut stdout = std::io::stdout().lock();
        if let Err(e) = stdout.write_all(&bytes).and_then(|()| stdout.flush()) {
            operational_error(&format!("writing the tile to stdout: {e}"));
        }
        return;
    }
    if let Err(e) = std::fs::write(&at.out, &bytes) {
        operational_error(&format!("writing the tile to {}: {e}", at.out));
    }
    eprintln!("Wrote {} bytes to {}", bytes.len(), at.out);
}
