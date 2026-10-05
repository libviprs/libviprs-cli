//! `viprs verify <pyramid-or-archive>` (libviprs-cli#66).
//!
//! The core has three ways to check a finished pyramid and this command is a
//! thin route onto them, picked by what the path is:
//!
//! * **an archive** goes to `verify::pyramid_verify`, which checks the
//!   archive's own account of itself against the plan, walks its directories,
//!   probes every planned tile and counts what it addresses. That proves every
//!   tile is present and non-empty and nothing else is there. It does not read
//!   a payload's contents, so a flipped byte inside a tile sails through, and
//!   this command then decodes every tile as well, which is what catches it.
//! * **a tile tree** is walked here for presence (every planned tile is a
//!   non-empty file), then goes to `checksum::verify_output`, which re-hashes
//!   every tile against the per-tile digests `viprs pyramid --checksum`
//!   recorded. `pyramid_verify` cannot take a tree: its last check counts the
//!   tiles the storage addresses, and `DirectoryPyramidReader` refuses to
//!   count, so the presence half is done by walking the plan instead.
//! * **a tile tree with `--source`** also goes to
//!   `stream_verify::verify_from_strip_source`, which re-renders the pyramid
//!   from the input and compares. It is the only check that can tell a tree
//!   was made from a different image, and only for `--format raw` trees does
//!   it compare bytes: an encoded tile cannot be re-encoded bit for bit, so for
//!   PNG and JPEG it falls back to the manifest digests above.
//!
//! # Centring and dropped blanks
//!
//! The manifest and the archive's `vnd.libviprs` metadata record whether the
//! plan was centred and whether the run dropped its blank tiles
//! (`GenerationSettings::centre` and `skip_blanks`, libviprs#1162), and the
//! plan is rebuilt with both. A pyramid written before those fields existed
//! reads them as `false`, so `--centre` and `--drop-blanks` stay as overrides
//! for it. They only ever add: a flag the pyramid already records changes
//! nothing, and no flag can make a pyramid that records centring or dropped
//! blanks read as one that did not. A missing tile on a pyramid that records
//! neither says which flag to try.
//!
//! The re-render lays a source out on a centred grid (libviprs#1163) and,
//! told the run skipped blanks, accepts a planned tile that is absent when
//! its re-render is blank (libviprs#1174), so `--source` checks a centred
//! tree and a `--drop-blanks` one as well as a plain one. A missing tile
//! whose re-render has content still fails.
//!
//! Every failure names the tile. A verify that says "corrupt" and not where is
//! a verify somebody has to rerun by hand to use. A pyramid with thousands of
//! problems (or a manifest that claims a much bigger source than the tree
//! holds) is reported up to [`MAX_PROBLEMS`] and then stops looking.

use std::path::{Path, PathBuf};
use std::process;

use libviprs::checksum::verify_output;
use libviprs::observe::NoopObserver;
use libviprs::sink::BLANK_TILE_MARKER;
use libviprs::sink_pmtiles::tile_coord_to_zxy;
use libviprs::stream_verify::verify_from_strip_source;
use libviprs::streaming::RasterStripSource;
use libviprs::verify::pyramid_verify;
use libviprs::{
    EngineConfig, EngineError, FsSink, Manifest, PmTilesPyramidReader, PyramidPlan, PyramidPlanner,
    PyramidReader, TileCoord, TileFormat,
};

use crate::{input, operational_error, usage_error};

/// How many problems one run lists before it stops looking.
const MAX_PROBLEMS: usize = 50;

/// Arguments of `viprs verify`.
#[derive(clap::Parser)]
pub(crate) struct VerifyArgs {
    /// A `.pmtiles` archive, or a tile tree written with a manifest
    /// (`viprs pyramid ... --storage directory --checksum`).
    pub(crate) path: PathBuf,

    /// Re-render from this input and compare it against the tree.
    ///
    /// Tile trees only. Compares bytes for `--format raw` trees; for encoded
    /// tiles the manifest digests are the check. Not a PDF: the re-render
    /// would need the page, DPI and render mode the pyramid used.
    #[arg(long, value_name = "FILE")]
    pub(crate) source: Option<PathBuf>,

    /// The pyramid was written with `--centre`, for one that does not say so.
    ///
    /// A pyramid records centring itself, so this is only for one written
    /// before it did, which otherwise gets checked against the uncentred
    /// grid. It can add centring, never take it away.
    #[arg(long)]
    pub(crate) centre: bool,

    /// The pyramid was written with `--drop-blanks`.
    ///
    /// A planned tile that is absent is then a dropped blank rather than a
    /// missing tile; every tile that is there is still checked. A pyramid
    /// records this itself, so the flag is only for one written before it
    /// did.
    #[arg(long)]
    pub(crate) drop_blanks: bool,
}

/// `viprs verify`: exit 0 and a summary on stdout, or exit 1 naming what is
/// wrong on stderr.
pub(crate) fn run(args: VerifyArgs) {
    if let Some(source) = &args.source
        && is_pdf(source)
    {
        usage_error(
            &format!(
                "{} is a PDF, and verify cannot re-render one",
                source.display()
            ),
            "the re-render needs the page, DPI and render mode the pyramid used, and \
             verify has none of them. Extract the page as an image first \
             (`viprs pdf extract`) and pass that",
        );
    }
    let told = Flags {
        centre: args.centre,
        drop_blanks: args.drop_blanks,
    };
    if args.path.is_dir() {
        verify_tree(&args.path, args.source.as_deref(), told);
    } else if args.path.is_file() {
        if args.source.is_some() {
            usage_error(
                "--source applies to a tile tree, not an archive",
                "an archive is checked by reading every tile back and decoding it; drop --source",
            );
        }
        verify_archive(&args.path, told);
    } else {
        operational_error(&format!("{} does not exist", args.path.display()));
    }
}

/// Whether the plan was centred and the run dropped its blank tiles.
#[derive(Clone, Copy)]
struct Flags {
    centre: bool,
    drop_blanks: bool,
}

impl Flags {
    /// What the pyramid records, plus what the command line adds for one
    /// written before it recorded them. Either side saying yes is a yes.
    fn with_recorded(self, centre: bool, skip_blanks: bool) -> Self {
        Self {
            centre: self.centre || centre,
            drop_blanks: self.drop_blanks || skip_blanks,
        }
    }
}

/// A `.pdf` by name or by its `%PDF-` header.
fn is_pdf(path: &Path) -> bool {
    let by_name = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    let by_header = || {
        use std::io::Read as _;
        let mut head = [0u8; 5];
        std::fs::File::open(path)
            .and_then(|mut f| f.read_exact(&mut head))
            .is_ok_and(|()| &head == b"%PDF-")
    };
    by_name || by_header()
}

/// The problems one run has found, up to [`MAX_PROBLEMS`].
struct Problems {
    list: Vec<String>,
    /// Whether a planned tile was missing, which on a pyramid that records
    /// neither is the sign of an older `--drop-blanks` or `--centre` run.
    missing: bool,
    full: bool,
}

impl Problems {
    fn new() -> Self {
        Self {
            list: Vec::new(),
            missing: false,
            full: false,
        }
    }

    /// Record one. `false` once the cap is reached: stop looking.
    fn push(&mut self, problem: String) -> bool {
        if self.list.len() < MAX_PROBLEMS {
            self.list.push(problem);
            true
        } else {
            self.full = true;
            false
        }
    }

    fn push_missing(&mut self, problem: String) -> bool {
        self.missing = true;
        self.push(problem)
    }

    /// Exit 1 listing every problem, one per line, if there is any.
    fn exit_if_any(self, flags: Flags) {
        if self.list.is_empty() {
            return;
        }
        for p in &self.list {
            eprintln!("Error: {p}");
        }
        if self.full {
            eprintln!("Error: stopped after {MAX_PROBLEMS} problems; there are more");
        }
        if self.missing {
            missing_hint(flags);
        }
        process::exit(1);
    }
}

/// The hint for a missing tile on a pyramid that does not record dropped
/// blanks or centring.
fn missing_hint(flags: Flags) {
    if !flags.drop_blanks || !flags.centre {
        eprintln!(
            "Hint: a pyramid written with --drop-blanks leaves blank tiles out, and one \
             written with --centre sits on a different grid. This one records neither, \
             which is what one written before libviprs recorded them looks like; if it \
             was written with either, pass the same flag to verify"
        );
    }
}

fn plan_for(
    what: &Path,
    width: u32,
    height: u32,
    tile_size: u32,
    overlap: u32,
    layout: libviprs::Layout,
    centre: bool,
) -> PyramidPlan {
    match PyramidPlanner::new(width, height, tile_size, overlap, layout) {
        Ok(p) => p.with_centre(centre).plan(),
        Err(e) => operational_error(&format!(
            "{} records a pyramid no plan can describe: {e}",
            what.display()
        )),
    }
}

fn verify_archive(path: &Path, told: Flags) {
    let reader = PmTilesPyramidReader::try_open(path)
        .unwrap_or_else(|e| operational_error(&format!("reading {}: {e}", path.display())));
    // The archive's own generation block. An archive without one is refused
    // just below for having no plan, so only its two flags matter here.
    let flags = match reader.reader().metadata() {
        Ok(meta) => match meta
            .vnd_libviprs
            .as_ref()
            .and_then(|l| l.generation.as_ref())
        {
            Some(g) => told.with_recorded(g.centre, g.skip_blanks),
            None => told,
        },
        Err(e) => operational_error(&format!("reading {}: {e}", path.display())),
    };
    let desc = reader
        .describe()
        .unwrap_or_else(|e| operational_error(&format!("reading {}: {e}", path.display())));
    let (Some(w), Some(h), Some(tile_size), Some(layout)) = (
        desc.source_width,
        desc.source_height,
        desc.tile_size,
        desc.layout,
    ) else {
        operational_error(&format!(
            "{} does not say how it was generated (no vnd.libviprs metadata), so there is no \
             plan to verify it against. `viprs pmtiles verify` checks its structure alone",
            path.display()
        ));
    };
    let plan = plan_for(
        path,
        w,
        h,
        tile_size,
        desc.overlap.unwrap_or(0),
        layout,
        flags.centre,
    );

    let checks = if flags.drop_blanks {
        // `pyramid_verify` probes every planned tile and wants the archive to
        // address exactly the plan, so a dropped blank fails it. The same
        // questions, asked with absence allowed: the structure is sound, and
        // the archive addresses exactly the planned tiles it holds.
        let held = verify_archive_tiles(&reader, &plan, flags);
        let structure = reader
            .structural_summary()
            .unwrap_or_else(|e| fail_one(&format!("{}: {e}", path.display())));
        let addressed = match structure.addressed_tiles {
            Some(n) => n,
            None => reader
                .addressed_tiles()
                .unwrap_or_else(|e| fail_one(&format!("{}: {e}", path.display()))),
        };
        if held == 0 {
            fail_one(&format!(
                "{} holds none of the planned tiles",
                path.display()
            ));
        }
        if addressed != held {
            fail_one(&format!(
                "{} addresses {addressed} tiles and {held} of them are planned ones; the \
                 rest are tiles this plan does not name",
                path.display()
            ));
        }
        "plan, structure, every tile present decoded (blank tiles dropped)"
    } else {
        if let Err(e) = pyramid_verify(&reader, &plan, desc.format, &NoopObserver) {
            let message = e.to_string();
            eprintln!("Error: {}: {message}", path.display());
            if message.contains("missing tile") || message.contains("addresses") {
                missing_hint(flags);
            }
            process::exit(1);
        }
        // pyramid_verify proves presence and length. The bytes are this pass.
        verify_archive_tiles(&reader, &plan, flags);
        "plan, structure, every tile present and decoded"
    };

    println!("Verified: {}", path.display());
    println!("Tiles: {}", plan.total_tile_count());
    println!("Checks: {checks}");
}

/// Decode every planned tile the archive holds and count them. An absent tile
/// is a problem unless the pyramid dropped its blanks.
fn verify_archive_tiles(reader: &PmTilesPyramidReader, plan: &PyramidPlan, flags: Flags) -> u64 {
    let mut problems = Problems::new();
    let mut held = 0u64;
    for coord in plan.tile_coords() {
        let name = zxy_name(coord);
        let more = match reader.tile(coord) {
            Ok(Some(bytes)) => {
                held += 1;
                if bytes.is_empty() {
                    problems.push(format!("tile {name} is stored with no bytes"))
                } else if bytes.as_slice() == [BLANK_TILE_MARKER] {
                    true
                } else if let Err(e) = input::decode_bytes_default(&bytes) {
                    problems.push(format!("tile {name} does not decode: {e:#}"))
                } else {
                    true
                }
            }
            Ok(None) if flags.drop_blanks => true,
            Ok(None) => problems.push_missing(format!("tile {name} is missing")),
            Err(e) => problems.push(format!("tile {name} cannot be read: {e}")),
        };
        if !more {
            break;
        }
    }
    problems.exit_if_any(flags);
    held
}

/// Exit 1 with one problem.
fn fail_one(problem: &str) -> ! {
    eprintln!("Error: {problem}");
    process::exit(1);
}

/// `z/x/y` for an archive tile, which is how PMTiles names it.
fn zxy_name(coord: TileCoord) -> String {
    match tile_coord_to_zxy(coord) {
        Ok((z, x, y)) => format!("{z}/{x}/{y}"),
        Err(_) => format!("{}/{}/{}", coord.level, coord.col, coord.row),
    }
}

/// The manifest inside or beside a tree, wherever `FsSink` wrote it
/// (`Manifest::locate`, libviprs#1169).
fn read_manifest(dir: &Path) -> Option<Manifest> {
    let path = Manifest::locate(dir)?;
    match Manifest::read_from(&path) {
        Ok(m) => Some(m),
        Err(e) => operational_error(&format!("reading {}: {e}", path.display())),
    }
}

fn verify_tree(dir: &Path, source: Option<&Path>, told: Flags) {
    let Some(manifest) = read_manifest(dir) else {
        operational_error(&format!(
            "{} has no manifest.json, so there is no plan to verify it against. Write the tree \
             with `viprs pyramid ... --storage directory --checksum`",
            dir.display()
        ));
    };
    let m = manifest.as_v1();
    let g = &m.generation;
    let format: TileFormat = g.format;
    let flags = told.with_recorded(g.centre, g.skip_blanks);
    let plan = plan_for(
        dir,
        m.source.width,
        m.source.height,
        g.tile_size,
        g.overlap,
        g.layout,
        flags.centre,
    );

    let mut problems = Problems::new();
    for coord in plan.tile_coords() {
        let Some(rel) = plan.tile_path(coord, format.extension()) else {
            continue;
        };
        let more = match std::fs::metadata(dir.join(&rel)) {
            Ok(meta) if meta.len() > 0 => true,
            Ok(_) => problems.push(format!("tile {rel} is empty")),
            Err(_) if flags.drop_blanks => true,
            Err(_) => problems.push_missing(format!("tile {rel} is missing")),
        };
        if !more {
            break;
        }
    }
    problems.exit_if_any(flags);

    let report = match verify_output(dir) {
        Ok(r) => r,
        Err(e) => fail_one(&format!("{}: {e}", dir.display())),
    };
    let mut problems = Problems::new();
    for p in &report.tiles_mismatched {
        if !problems.push(format!("tile {} does not match its checksum", p.display())) {
            break;
        }
    }
    for p in &report.tiles_missing {
        if !problems.push_missing(format!("tile {} is missing", p.display())) {
            break;
        }
    }
    problems.exit_if_any(flags);

    let mut checks = vec!["plan", "every tile present"];
    if flags.drop_blanks {
        checks[1] = "every kept tile present (blank tiles dropped)";
    }
    if report.tiles_checked > 0 {
        checks.push("every tile against its checksum");
    }

    if let Some(source) = source {
        rerender(dir, source, &plan, format, m, flags);
        checks.push("a re-render of the source");
    }

    println!("Verified: {}", dir.display());
    println!("Tiles: {}", plan.total_tile_count());
    println!("Checks: {}", checks.join(", "));
    if report.tiles_checked == 0 {
        println!(
            "Note: the manifest records no per-tile checksums, so tile contents were not \
             checked. Write the tree with --checksum to get them."
        );
    }
}

/// Re-render the pyramid from `source` and compare it against the tree.
///
/// The source decodes through [`input::decode_path_default`], the one input
/// path every command uses, so SVG routing, the decode limits and the
/// missing-feature refusal are the same here as in `viprs pyramid`.
fn rerender(
    dir: &Path,
    source: &Path,
    plan: &PyramidPlan,
    format: TileFormat,
    m: &libviprs::ManifestV1,
    flags: Flags,
) {
    let raster = input::decode_path_default(source)
        .unwrap_or_else(|e| operational_error(&format!("decoding {}: {e:#}", source.display())));
    if (raster.width(), raster.height()) != (plan.image_width, plan.image_height) {
        fail_one(&format!(
            "{} is {}x{}, and the tree was made from a {}x{} image",
            source.display(),
            raster.width(),
            raster.height(),
            plan.image_width,
            plan.image_height
        ));
    }
    let strips = RasterStripSource::new(&raster);
    let sink = FsSink::new(dir, plan.clone()).with_format(format);
    let mut config = EngineConfig::default()
        .with_blank_tile_strategy(m.generation.blank_strategy)
        .skip_blanks(flags.drop_blanks);
    config.background_rgb = m.generation.background_rgb;
    // `viprs pyramid` hands the core the BLAKE3 of the input file on every
    // tree run from a file, and the core folds it into the plan hash the
    // checkpoint records (#88), so the re-render has to fold in the same
    // digest. A tree written from stdin, or before #88, has none, so a
    // mismatch with the digest gets one more try without it.
    let digest = crate::pipeline::hash_source_file(source)
        .unwrap_or_else(|e| operational_error(&format!("hashing {}: {e}", source.display())));
    let hashed = config.clone().with_source_content_hash(digest);
    let mut outcome = verify_from_strip_source(&strips, plan, &sink, &hashed, &NoopObserver);
    if matches!(outcome, Err(EngineError::PlanHashMismatch { .. })) {
        outcome = verify_from_strip_source(&strips, plan, &sink, &config, &NoopObserver);
    }
    let problem = match outcome {
        Ok(_) => return,
        Err(EngineError::ChecksumMismatch { tile, .. }) => {
            let name = plan
                .tile_path(tile, format.extension())
                .unwrap_or_else(|| format!("{tile:?}"));
            format!(
                "tile {name} does not match a re-render of {}",
                source.display()
            )
        }
        // The checkpoint records the hash of the plan the run used, with the
        // source file's digest folded in. So a different source file gives
        // itself away here, and so does a centred tree whose manifest
        // predates `centre`.
        Err(e @ EngineError::PlanHashMismatch { .. }) => format!(
            "{} was made from a different source than {}, or with a different plan ({e})",
            dir.display(),
            source.display()
        ),
        Err(e) => fail_one(&format!("{}: {e}", dir.display())),
    };
    eprintln!("Error: {problem}");
    if !flags.centre {
        eprintln!(
            "Hint: a pyramid written with --centre sits on a different grid. This tree's \
             manifest does not record centring, which is what one written before libviprs \
             recorded it looks like; if it was written with --centre, pass --centre"
        );
    }
    process::exit(1);
}
