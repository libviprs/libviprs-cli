//! `viprs vector` (libviprs#1035): the command line over the library's vector
//! path.
//!
//! ```text
//! viprs vector views    <drawing.dwg>               list the drawing's views
//! viprs vector build    <drawing.dwg> [out.pmtiles]  one view to a vector PMTiles archive
//! viprs vector report   <drawing.dwg>               the same run, no file: fidelity and budgets
//! viprs vector info     <archive.pmtiles>           what an archive says about itself
//! viprs vector validate <archive.pmtiles>           is it a sound vector archive
//! ```
//!
//! Every command is a thin call onto `libviprs::vector::convert`, which is the
//! one engine: this file parses arguments, installs the Ctrl-C handler, prints
//! what the library returns and picks an exit code. It has no tiling, no
//! archive handling and no validation of its own, so a result here is the
//! result a Rust caller gets.
//!
//! # Exit codes
//!
//! The contract in the README applies unchanged: 0 success, 1 operational
//! failure, 2 usage mistake, 130 interrupted. Specifically for this family:
//!
//! * a build without the `dwg-acadsharp` feature cannot read a DWG, and says so
//!   with exit 1 and the feature to rebuild with, as a missing codec does.
//!   `info` and `validate` read a PMTiles file and work in every build.
//! * `validate` exits 1 when it finds something wrong with the archive.
//! * `build` and `report` stopped by Ctrl-C exit 130, and one stopped by
//!   `--timeout` exits 1. Either way nothing is left at the destination, and no
//!   staging files beside it, because the library removes them on its way out.
//!   A second Ctrl-C quits at once and cannot clean up; a process that is
//!   killed outright can leave `<output>.tmp*` staging files, and never a
//!   half-written output.

use std::path::{Path, PathBuf};
use std::time::Duration;

use libviprs::CancelToken;
use libviprs::cad::{CadDrawing, CadError, CadSource, ViewKind};
use libviprs::vector::convert::{
    Conversion, ConvertError, ConvertOptions, ViewSelector, check, convert, default_decoder,
    inspect, list_views, report, view_json,
};
use libviprs::vector::tiler::{TilerConfig, TilerError};
use serde_json::json;

use crate::{operational_error, pipeline, usage_error};

/// Arguments of `viprs vector`.
#[derive(clap::Parser)]
pub(crate) struct VectorArgs {
    #[command(subcommand)]
    command: VectorCommand,
}

#[derive(clap::Subcommand)]
enum VectorCommand {
    /// List the views of a DWG: Model Space and each paper-space layout.
    ///
    /// Only Model Space can be built until paper space has its viewport
    /// fixtures, and a layout is listed with that said. Needs a build with the
    /// `dwg-acadsharp` feature.
    Views(ViewsArgs),

    /// Convert one view of a DWG to a vector PMTiles archive.
    ///
    /// The archive is written beside the output and renamed into place only
    /// when it is complete, so a failed, interrupted or timed out run leaves
    /// nothing at the output (an existing file there is untouched). With no
    /// output the archive takes the input's name with a `.pmtiles` extension.
    /// Needs a build with the `dwg-acadsharp` feature.
    #[command(alias = "convert")]
    Build(BuildArgs),

    /// Do a build's work and write nothing: the fidelity numbers, the primitive
    /// counts, every diagnostic and the budget counters.
    ///
    /// Takes the same view and tiler flags as `build` and runs the same code,
    /// so what it reports is what a build would have done. Needs a build with
    /// the `dwg-acadsharp` feature.
    Report(ReportArgs),

    /// Show what a vector PMTiles archive says about itself: zoom range,
    /// bounds, tile counts, layers and the tiler's own metadata.
    ///
    /// Reads a `.pmtiles` file, so it works in every build.
    Info(ArchiveArgs),

    /// Check a vector PMTiles archive: the PMTiles structure, MVT tiles, and
    /// a valid `vector_layers`.
    ///
    /// Exit 0 when it is sound and 1 when it is not, with every finding on
    /// stderr. Reads a `.pmtiles` file, so it works in every build.
    Validate(ArchiveArgs),
}

/// The flags every command that opens a drawing takes.
#[derive(clap::Args)]
struct DrawingArgs {
    /// The drawing (a DWG).
    input: PathBuf,

    /// Open the drawing under the hardened provider limits: a quarter of a
    /// gigabyte of input, ten million entities, bounded strings, polylines,
    /// nesting depth and output.
    ///
    /// Use it for a drawing somebody else made. The default sets no bound,
    /// which is right for your own files. The reader is still not a sandbox,
    /// so an upload also wants a process with its own memory and time ceiling.
    #[arg(long)]
    hardened: bool,

    /// Print one JSON object on stdout instead of text.
    #[arg(long)]
    json: bool,
}

#[derive(clap::Args)]
struct ViewsArgs {
    #[command(flatten)]
    drawing: DrawingArgs,
}

/// The flags `build` and `report` share, because `report` is a build that
/// writes nothing.
#[derive(clap::Args)]
struct RunArgs {
    #[command(flatten)]
    drawing: DrawingArgs,

    /// Which view: `model` (the default), a view's index, or its name.
    ///
    /// `viprs vector views` lists them. A layout is refused for now, with the
    /// reason, and a name that matches nothing lists the names there are.
    #[arg(long, value_name = "VIEW", default_value = "model")]
    view: String,

    /// The deepest zoom generated. Zoom 0 is one tile holding the drawing, and
    /// each level doubles it. Deeper is sharper and larger.
    #[arg(long, value_name = "ZOOM", default_value_t = 12, value_parser = clap::value_parser!(u8).range(0..=24))]
    max_zoom: u8,

    /// Stop after this long and leave nothing behind. A bare number is
    /// seconds, and `ms`, `s`, `m` and `h` are understood: `90`, `500ms`, `2m`.
    ///
    /// The clock covers the decode and the tiling. The native reader cannot be
    /// interrupted inside one record, so the run stops at the first primitive
    /// after the deadline, which is soon but not instant. Exit 1.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    timeout: Option<Duration>,
}

#[derive(clap::Args)]
struct BuildArgs {
    #[command(flatten)]
    run: RunArgs,

    /// Where the archive goes. The input's name with `.pmtiles` when omitted.
    output: Option<PathBuf>,

    /// The archive's name, as a map client shows it. The view's name when
    /// omitted.
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
}

#[derive(clap::Args)]
struct ReportArgs {
    #[command(flatten)]
    run: RunArgs,
}

#[derive(clap::Args)]
struct ArchiveArgs {
    /// A vector `.pmtiles` archive.
    archive: PathBuf,

    /// Print one JSON object on stdout instead of text.
    #[arg(long)]
    json: bool,
}

/// The version of every `--json` shape in this family. A script that knows `1`
/// can tell a later shape from the one it was written for.
const JSON_VERSION: u32 = 1;

/// `1`, `90`, `500ms`, `2m`: a positive span.
fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let value: f64 = number
        .parse()
        .map_err(|_| format!("{text:?} is not a duration; write 90, 500ms, 2m or 1h"))?;
    let seconds = match unit.trim() {
        "" | "s" => value,
        "ms" => value / 1000.0,
        "m" => value * 60.0,
        "h" => value * 3600.0,
        other => return Err(format!("{other:?} is not a unit; use ms, s, m or h")),
    };
    if !seconds.is_finite() || seconds <= 0.0 {
        return Err("a timeout has to be longer than zero".to_owned());
    }
    // A day is longer than any conversion is meant to run, and a value past it
    // is a typo that would otherwise overflow `Duration`.
    if seconds > 86_400.0 {
        return Err("a timeout is at most 24h".to_owned());
    }
    Ok(Duration::from_secs_f64(seconds))
}

/// Why a run stopped, as the command line sees it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// A mistake in what was typed (exit 2).
    Usage(String),
    /// Something the input or the environment decided (exit 1).
    Operational(String),
    /// Ctrl-C (exit 130).
    Interrupted,
}

impl Failure {
    /// The process exit status.
    pub(crate) fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            Self::Operational(_) => 1,
            Self::Interrupted => 130,
        }
    }
}

/// Sorts a library error into the exit-code contract.
pub(crate) fn classify(error: &ConvertError, timeout: Option<Duration>) -> Failure {
    if error.is_cancelled() {
        return Failure::Interrupted;
    }
    if error.is_deadline() {
        let limit = timeout.map_or_else(String::new, |t| format!(" {t:?}"));
        return Failure::Operational(format!(
            "the run passed its{limit} --timeout; nothing was written"
        ));
    }
    match error {
        ConvertError::ProviderNotBuilt => Failure::Operational(
            "this build has no DWG provider, so it cannot read a drawing. Rebuild with \
             `--features dwg-acadsharp` (`vector info` and `vector validate` still work)"
                .to_owned(),
        ),
        // A selector the command line alone gets wrong.
        ConvertError::NoSuchViewName { .. } | ConvertError::NoModelSpace => {
            Failure::Usage(error.to_string())
        }
        ConvertError::Cad(CadError::NoSuchView { .. }) => Failure::Usage(error.to_string()),
        ConvertError::Tiler(TilerError::ViewNotEnabled { .. }) => Failure::Usage(error.to_string()),
        ConvertError::Tiler(TilerError::Decode(CadError::NoSuchView { .. })) => {
            Failure::Usage(error.to_string())
        }
        _ => Failure::Operational(error.to_string()),
    }
}

fn fail(failure: Failure) -> ! {
    match &failure {
        Failure::Usage(message) => usage_error(
            message,
            "`viprs vector views <drawing>` lists the views, and `--help` the flags",
        ),
        Failure::Operational(message) => operational_error(message),
        Failure::Interrupted => {
            eprintln!("Interrupted: nothing was written.");
            std::process::exit(failure.exit_code());
        }
    }
}

/// `viprs vector ...`.
pub(crate) fn run(args: VectorArgs) {
    match args.command {
        VectorCommand::Views(a) => run_views(&a),
        VectorCommand::Build(a) => run_build(a),
        VectorCommand::Report(a) => run_report(&a),
        VectorCommand::Info(a) => run_info(&a),
        VectorCommand::Validate(a) => run_validate(&a),
    }
}

/// Opens the drawing through the library's own provider choice.
fn with_drawing<T>(args: &DrawingArgs, then: impl FnOnce(&dyn CadDrawing) -> T) -> T {
    if !args.input.exists() {
        operational_error(&format!("{} does not exist", args.input.display()));
    }
    let decoder = match default_decoder(args.hardened) {
        Ok(d) => d,
        Err(e) => fail(classify(&e, None)),
    };
    let drawing = match decoder.open(CadSource::Path(&args.input)) {
        Ok(d) => d,
        Err(e) => operational_error(&format!("{}: {e}", args.input.display())),
    };
    then(drawing.as_ref())
}

fn run_views(args: &ViewsArgs) {
    with_drawing(&args.drawing, |drawing| {
        let views = match list_views(drawing) {
            Ok(v) => v,
            Err(e) => fail(classify(&e, None)),
        };
        if args.drawing.json {
            let list: Vec<_> = views.iter().map(view_json).collect();
            println!("{}", json!({ "v": JSON_VERSION, "views": list }));
            return;
        }
        for view in &views {
            let kind = match view.kind() {
                ViewKind::Model => "model",
                ViewKind::Layout => "layout",
                _ => "unknown",
            };
            // A zero-size box is a header nobody filled in, which is not an
            // extent worth printing.
            let extents = view
                .extents()
                .filter(|e| e.max_x() > e.min_x() || e.max_y() > e.min_y())
                .map_or_else(
                    || "no extents".to_owned(),
                    |e| {
                        format!(
                            "{} x {} drawing units",
                            short(e.max_x() - e.min_x()),
                            short(e.max_y() - e.min_y())
                        )
                    },
                );
            let note = if view.kind() == ViewKind::Model {
                ""
            } else {
                "  (paper space is not enabled yet)"
            };
            println!(
                "{:>3}  {kind:<6}  {:<24}  {} entities, {extents}{note}",
                view.index(),
                format!("{:?}", view.name()),
                view.entity_count()
            );
        }
    });
}

fn tiler_config(args: &RunArgs, cancel: Option<CancelToken>) -> TilerConfig {
    let mut config = TilerConfig::default().with_max_zoom(args.max_zoom);
    if let Some(t) = args.timeout {
        config = config.with_deadline(t);
    }
    if let Some(c) = cancel {
        config = config.with_cancel(c);
    }
    config
}

/// What `build` does once it has a drawing. Separate from [`run_build`] so a
/// test can hand it a drawing that needs no provider.
pub(crate) fn build_with(
    drawing: &dyn CadDrawing,
    view: &ViewSelector,
    output: &Path,
    options: &ConvertOptions,
) -> Result<Conversion, ConvertError> {
    convert(drawing, view, output, options)
}

fn run_build(args: BuildArgs) {
    let cancel = pipeline::install_sigint();
    let output = args
        .output
        .clone()
        .unwrap_or_else(|| args.run.drawing.input.with_extension("pmtiles"));
    if output == args.run.drawing.input {
        usage_error(
            "the output is the input",
            "give an output path, or an input that does not end in .pmtiles",
        );
    }
    let mut options = ConvertOptions::new().with_tiler(tiler_config(&args.run, Some(cancel)));
    if let Some(name) = &args.name {
        options = options.with_name(name.clone());
    }
    let selector = ViewSelector::parse(&args.run.view);
    let done = with_drawing(&args.run.drawing, |drawing| {
        build_with(drawing, &selector, &output, &options)
    });
    match done {
        Ok(done) => print_conversion(&done, args.run.drawing.json),
        Err(e) => fail(classify(&e, args.run.timeout)),
    }
}

fn run_report(args: &ReportArgs) {
    let cancel = pipeline::install_sigint();
    let options = ConvertOptions::new().with_tiler(tiler_config(&args.run, Some(cancel)));
    let selector = ViewSelector::parse(&args.run.view);
    let done = with_drawing(&args.run.drawing, |drawing| {
        report(drawing, &selector, &options)
    });
    match done {
        Ok(done) => print_conversion(&done, args.run.drawing.json),
        Err(e) => fail(classify(&e, args.run.timeout)),
    }
}

/// How many diagnostics the text form lists before it points at `--json`.
const TEXT_DIAGNOSTICS: usize = 20;

fn print_conversion(done: &Conversion, as_json: bool) {
    if as_json {
        let mut j = done.to_json();
        j["v"] = json!(JSON_VERSION);
        println!("{j}");
        return;
    }
    let t = &done.tiler;
    match &done.destination {
        Some(dest) => println!(
            "Wrote {} ({} bytes)",
            dest.display(),
            done.bytes.unwrap_or(0)
        ),
        None => println!("Report only, nothing written"),
    }
    println!(
        "View:        {:?} (index {})",
        done.view.name(),
        done.view.index()
    );
    println!(
        "Primitives:  {} decoded, {} drawable",
        t.primitives, t.drawable
    );
    println!(
        "Tiles:       {} written over zoom 0 to {}, largest {} bytes",
        t.tiles,
        t.tiles_per_zoom.len().saturating_sub(1),
        t.largest_tile_bytes
    );
    let notes: Vec<(&str, u64)> = [
        ("undrawable primitives", t.undrawable),
        ("curves stopped by the point cap", t.curves_capped),
        ("tiles past the byte budget", t.oversized_count),
        (
            "runs dropped for the point budget",
            t.dropped_for_point_budget,
        ),
        (
            "text dropped for the text budget",
            t.dropped_for_text_budget,
        ),
        ("labels cut to the length cap", t.text_truncated),
    ]
    .into_iter()
    .filter(|(_, n)| *n > 0)
    .collect();
    for (what, n) in &notes {
        println!("Fidelity:    {n} {what}");
    }
    if !t.fonts_substituted.is_empty() {
        println!(
            "Fidelity:    {} font(s) substituted by the one font the output has",
            t.fonts_substituted.len()
        );
    }
    if !t.glyphs_unavailable.is_empty() {
        println!(
            "Fidelity:    {} character(s) the served glyph files cannot draw",
            t.glyphs_unavailable.len()
        );
    }
    let diagnostics: Vec<String> = done
        .decode
        .diagnostics()
        .iter()
        .chain(t.diagnostics.iter())
        .map(|d| format!("{d}"))
        .collect();
    if !diagnostics.is_empty() {
        println!("Diagnostics: {}", diagnostics.len());
        for line in diagnostics.iter().take(TEXT_DIAGNOSTICS) {
            println!("  {line}");
        }
        if diagnostics.len() > TEXT_DIAGNOSTICS {
            println!(
                "  and {} more (--json lists them all)",
                diagnostics.len() - TEXT_DIAGNOSTICS
            );
        }
    }
    if !done.decode.is_complete() {
        eprintln!(
            "Warning: the decoder did not report a complete decode, so the output may be missing \
             entities. `--json` has the details."
        );
    }
}

fn run_info(args: &ArchiveArgs) {
    check_exists(&args.archive);
    let info = match inspect(&args.archive) {
        Ok(i) => i,
        Err(e) => operational_error(&format!("{}: {e}", args.archive.display())),
    };
    if args.json {
        let mut j = info.to_json();
        j["v"] = json!(JSON_VERSION);
        println!("{j}");
        return;
    }
    let [w, s, e, n] = info.bounds;
    println!("Name:        {}", info.name.as_deref().unwrap_or("(none)"));
    println!(
        "Tiles:       {}, {} compressed",
        info.tile_type, info.tile_compression
    );
    println!("Zoom:        {} to {}", info.min_zoom, info.max_zoom);
    println!("Bounds:      {w:.6},{s:.6},{e:.6},{n:.6} (lon/lat degrees)");
    println!(
        "Entries:     {} ({} addressed, {} distinct)",
        info.tile_entries, info.addressed_tiles, info.tile_contents
    );
    if let Some(bytes) = info.file_bytes {
        println!("Size:        {bytes} bytes");
    }
    for layer in &info.layers {
        let fields: Vec<&str> = layer.fields.keys().map(String::as_str).collect();
        if fields.is_empty() {
            println!("Layer:       {}", layer.id);
        } else {
            println!("Layer:       {} ({})", layer.id, fields.join(", "));
        }
    }
    if let Some(t) = &info.tiler {
        println!("Tiler:       {t}");
    }
}

fn run_validate(args: &ArchiveArgs) {
    check_exists(&args.archive);
    let verdict = match check(&args.archive) {
        Ok(v) => v,
        Err(e) => operational_error(&format!("{}: {e}", args.archive.display())),
    };
    if args.json {
        let mut j = verdict.to_json();
        j["v"] = json!(JSON_VERSION);
        println!("{j}");
    } else if verdict.is_valid() {
        println!(
            "OK: {} is a sound vector archive ({} tile entries)",
            args.archive.display(),
            verdict.tile_entries
        );
    }
    if !verdict.is_valid() {
        if !args.json {
            for finding in &verdict.findings {
                eprintln!("Error: {finding}");
            }
        }
        std::process::exit(1);
    }
}

fn check_exists(path: &Path) {
    if !path.is_file() {
        operational_error(&format!("{} is not a file", path.display()));
    }
}

/// A drawing-unit length with the digits that matter.
fn short(v: f64) -> String {
    if v.abs() >= 100.0 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libviprs::cad::{CadView, DecodeReport, Line, PrimitiveKind, PrimitiveSink};

    /// A model space view of `rungs` lines. No provider, so these run in every
    /// build, which the subprocess cells cannot (they need a native archive).
    struct Ladder(u32);

    impl CadDrawing for Ladder {
        fn views(&self) -> Result<Vec<CadView>, CadError> {
            Ok(vec![CadView::new(
                0,
                0,
                [0.0, 0.0, 100.0, 100.0],
                1,
                "Model",
            )])
        }

        fn decode(
            &self,
            _view: u32,
            sink: &mut dyn PrimitiveSink,
            report: &mut DecodeReport,
        ) -> Result<(), CadError> {
            std::thread::sleep(Duration::from_millis(20));
            for i in 0..self.0 {
                let y = f64::from(i);
                sink.primitive(Line::new([0.0, y, 0.0], [100.0, y + 1.0, 0.0])?.into())?;
                report.record(PrimitiveKind::Line);
            }
            report.mark_complete();
            sink.finish()
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("viprs-vector-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn left_in(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    fn config(args: &[&str]) -> RunArgs {
        use clap::Parser;
        #[derive(clap::Parser)]
        struct Wrap {
            #[command(flatten)]
            run: RunArgs,
        }
        Wrap::parse_from(
            std::iter::once("t")
                .chain(args.iter().copied())
                .chain(["in.dwg"]),
        )
        .run
    }

    #[test]
    fn durations_read_the_way_the_help_says() {
        assert_eq!(parse_duration("90"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(parse_duration("2m"), Ok(Duration::from_secs(120)));
        assert_eq!(parse_duration("1h"), Ok(Duration::from_secs(3600)));
        assert_eq!(parse_duration("1.5s"), Ok(Duration::from_millis(1500)));
        for bad in ["0", "0ms", "", "soon", "5x", "-3", "100h"] {
            assert!(parse_duration(bad).is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn each_failure_has_the_contracts_exit_code() {
        assert_eq!(Failure::Usage(String::new()).exit_code(), 2);
        assert_eq!(Failure::Operational(String::new()).exit_code(), 1);
        assert_eq!(Failure::Interrupted.exit_code(), 130);
    }

    #[test]
    fn a_cancel_is_130_and_leaves_nothing_in_the_directory() {
        let dir = scratch("cancel");
        let out = dir.join("never.pmtiles");
        let token = CancelToken::new();
        let options = ConvertOptions::new().with_tiler(tiler_config(
            &config(&["--max-zoom", "6"]),
            Some(token.clone()),
        ));
        let stopper = {
            let token = token.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(5));
                token.cancel();
            })
        };
        let err = build_with(&Ladder(300), &ViewSelector::Model, &out, &options).unwrap_err();
        stopper.join().unwrap();
        assert_eq!(classify(&err, None), Failure::Interrupted, "{err:?}");
        assert_eq!(classify(&err, None).exit_code(), 130);
        assert!(left_in(&dir).is_empty(), "left: {:?}", left_in(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_timeout_is_1_names_the_flag_and_leaves_nothing() {
        let dir = scratch("timeout");
        let out = dir.join("never.pmtiles");
        let args = config(&["--max-zoom", "6", "--timeout", "1ms"]);
        let options = ConvertOptions::new().with_tiler(tiler_config(&args, None));
        let err = build_with(&Ladder(300), &ViewSelector::Model, &out, &options).unwrap_err();
        let failure = classify(&err, args.timeout);
        assert_eq!(failure.exit_code(), 1);
        let Failure::Operational(message) = failure else {
            panic!("expected an operational failure, got {err:?}");
        };
        assert!(message.contains("--timeout"), "{message}");
        assert!(left_in(&dir).is_empty(), "left: {:?}", left_in(&dir));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_finished_build_is_one_valid_archive_and_nothing_else() {
        let dir = scratch("done");
        let out = dir.join("a.pmtiles");
        let options =
            ConvertOptions::new().with_tiler(tiler_config(&config(&["--max-zoom", "4"]), None));
        let done = build_with(&Ladder(40), &ViewSelector::Model, &out, &options).unwrap();
        assert_eq!(done.destination.as_deref(), Some(out.as_path()));
        assert_eq!(left_in(&dir), vec!["a.pmtiles".to_owned()]);
        assert!(check(&out).unwrap().is_valid());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn what_the_command_line_alone_gets_wrong_is_a_usage_error() {
        let missing = ConvertError::NoSuchViewName {
            name: "x".into(),
            available: vec!["Model".into()],
        };
        assert_eq!(classify(&missing, None).exit_code(), 2);
        assert_eq!(classify(&ConvertError::NoModelSpace, None).exit_code(), 2);
        let layout = ConvertError::Tiler(TilerError::ViewNotEnabled {
            index: 1,
            name: "Sheet".into(),
        });
        assert_eq!(classify(&layout, None).exit_code(), 2);
    }

    #[test]
    fn a_missing_provider_names_the_feature_to_rebuild_with() {
        let Failure::Operational(m) = classify(&ConvertError::ProviderNotBuilt, None) else {
            panic!("a missing provider is operational");
        };
        assert!(m.contains("--features dwg-acadsharp"), "{m}");
    }
}
