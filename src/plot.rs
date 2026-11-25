//! `viprs plot` (libviprs-cli#157, the CLI half of libviprs#1670): a DWG's print layouts, or its model space,
//! plotted with a plot style table and a plotter config.
//!
//! ```text
//! viprs plot <drawing.dwg> <out.pdf>                          every print layout, one page each
//! viprs plot <drawing.dwg> <out.pdf> --ctb A.ctb --pc3 B.pc3  with a plot style table and a plotter
//! viprs plot <drawing.dwg> <out.pdf> --ctb none               the objects' own colours and widths
//! viprs plot <drawing.dwg> <out.pdf> --no-defaults            flags only: no variable, no stored default
//! viprs plot <drawing.dwg> <out-dir> --format pmtiles --dpi 200   one raster archive per sheet
//! viprs plot <drawing.dwg> <out-dir> --format pvec --layout A-101 only that layout (repeatable)
//! viprs plot <drawing.dwg> <out-dir> --format pvec --model        model space, one .pvec
//! viprs plot <drawing.dwg> <out-dir> --format pvec --model --layer-state Review
//! ```
//!
//! A thin call onto the core: `PdfPlot`, `PmTilesPlot` and `PvecLayoutSet` (`PvecConversion` for model space). The
//! design is the consensus comment on libviprs#1670 (D2, D3, D6). This file parses the arguments, refuses
//! what has no meaning before anything is written, reads the two plot files through the core's loaders
//! (`load_plot_style` and `load_pc3`, which hold a file to 1 MiB and what a CTB inflates to to 8 MiB) and
//! picks the exit code. Each of the two plot files walks its own precedence, the flag, then
//! `LIBVIPRS_PLOT_CTB` / `LIBVIPRS_PLOT_PC3`, then the stored default of `viprs plot-defaults`, then the
//! built-in one (libviprs-cli#162, `crate::plot_defaults`).
//!
//! # Exit codes
//!
//! The contract's, unchanged: 0 when everything asked for was written; 1 when a sheet failed, a plot file
//! was refused, a layout or layer state is not in the drawing, a limit or the deadline was hit, or the
//! build lacks a feature the format needs; 2 for a usage mistake (nothing written); 130 when Ctrl-C
//! stopped it, with nothing left behind but the sheets a PVEC set had already published.
//!
//! # What the core does not do yet
//!
//! `PdfPlot` and `PmTilesPlot` plot paper layouts only, so `--model` takes `--format pvec`. `PmTilesPlot` has no
//! switch for text and fills, so an archive is strokes only, as the core's own default is.

use std::path::PathBuf;

use clap::{Args, ValueEnum};

use crate::paths::same_file;
use crate::usage_error;

/// What `viprs plot` writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum Format {
    /// One PDF, a page per layout. OUT is the file.
    Pdf,
    /// One raster pyramid archive per layout, and an index. OUT is a directory. Needs the `pdfium` feature.
    Pmtiles,
    /// One .pvec per layout, and an index. OUT is a directory.
    Pvec,
}

/// A number of seconds above zero that a duration can hold.
fn parse_seconds(text: &str) -> Result<f64, String> {
    let n: f64 = text
        .parse()
        .map_err(|_| format!("{text:?} is not a number"))?;
    if n > 0.0 && std::time::Duration::try_from_secs_f64(n).is_ok() {
        Ok(n)
    } else {
        Err("the deadline is a number of seconds above zero".to_owned())
    }
}

/// `viprs plot`.
#[derive(Args)]
pub(crate) struct PlotArgs {
    /// The drawing (a DWG).
    drawing: PathBuf,

    /// Where the plot goes: the PDF file for `--format pdf`, a directory (created when it does not exist)
    /// for `pmtiles` and `pvec`. It is never the drawing, the CTB or the PC3, and nothing is written
    /// outside it.
    out: PathBuf,

    /// What to write. Without it the format is a PDF when OUT ends in `.pdf`, and anything else has to say.
    #[arg(long, value_enum)]
    format: Option<Format>,

    /// The plot style table (a .ctb), read through the core's loader: at most 1 MiB, and at most 8 MiB
    /// inflated. Without one the walk goes on to LIBVIPRS_PLOT_CTB, then the stored default
    /// (`viprs plot-defaults set`), then the built-in monochrome table. `--ctb none` plots the objects' own
    /// colours and widths.
    #[arg(long, value_name = "FILE")]
    ctb: Option<PathBuf>,

    /// The plotter config (a .pc3, JSON or binary). Its paper and printable area fill what a layout's
    /// own page setup leaves empty, and never replace what it says. Without one the walk goes on to
    /// LIBVIPRS_PLOT_PC3, then the stored default, then the built-in PDF device, and it needs no `--ctb`.
    #[arg(long, value_name = "FILE")]
    pc3: Option<PathBuf>,

    /// Skip LIBVIPRS_PLOT_CTB, LIBVIPRS_PLOT_PC3 and the stored default, so a run depends on its flags
    /// alone. `--ctb` and `--pc3` still apply.
    #[arg(long)]
    no_defaults: bool,

    /// Plot only this paper layout (repeatable), matched with ASCII case ignored. They come out in tab
    /// order. Without it every print layout is plotted.
    #[arg(long, value_name = "NAME", conflicts_with = "model")]
    layout: Vec<String>,

    /// Plot model space instead of the print layouts. The core plots it as a PVEC only, so this needs
    /// `--format pvec`. Without `--layer-state` the layer table as the drawing saved it decides what is drawn.
    #[arg(long)]
    model: bool,

    /// Apply this saved layer state under its mask before plotting, with its name matched with ASCII case
    /// ignored. A name the drawing does not store exits 1 and lists the ones it does.
    #[arg(long, value_name = "NAME")]
    layer_state: Option<String>,

    /// The file stem of the sheets in a directory (the drawing's own by default), a plain file name.
    #[arg(long)]
    stem: Option<String>,

    /// The drawing's unit when its header declares none (`mm`, `cm`, `m`, `in`, `ft`).
    #[arg(long, value_parser = ["mm", "cm", "m", "in", "ft"])]
    unit: Option<String>,

    /// The resolution a PMTiles sheet is rendered at, 36 to 600 (300 by default).
    #[arg(long, value_parser = clap::value_parser!(u32).range(36..=600))]
    dpi: Option<u32>,

    /// Print one line when each sheet starts and one when it is done, as `[2/64] A-101 started`, on stderr.
    #[arg(long)]
    progress: bool,

    /// Stop after this many seconds (a number above zero): exit 1 with the code `DEADLINE` and nothing
    /// written, except the sheets a PVEC set had already published.
    #[arg(long, value_name = "SECONDS", value_parser = parse_seconds)]
    deadline: Option<f64>,
}

/// A plain file name: what is joined onto OUT and onto nothing else.
fn plain_name(stem: &str) -> bool {
    !(stem.is_empty() || stem.contains(['/', '\\', '\0']) || stem == "." || stem == "..")
}

pub(crate) fn run(args: PlotArgs) {
    let format = args.format.unwrap_or_else(|| {
        if args
            .out
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            Format::Pdf
        } else {
            usage_error(
                &format!(
                    "{} is not a .pdf, so the format has to be named",
                    args.out.display()
                ),
                "give --format pdf, pmtiles or pvec",
            )
        }
    });
    if args.dpi.is_some() && format != Format::Pmtiles {
        usage_error(
            "--dpi only applies to --format pmtiles",
            "drop it, or plot PMTiles",
        );
    }
    if args.stem.is_some() && format == Format::Pdf {
        usage_error(
            "--stem only applies to a directory of sheets (--format pmtiles or pvec)",
            "a PDF is named by OUT",
        );
    }
    if let Some(stem) = &args.stem
        && !plain_name(stem)
    {
        usage_error(
            &format!("--stem {stem:?} is not a plain file name"),
            "give a name with no path separator, such as --stem plan",
        );
    }
    let out_is_file = format == Format::Pdf;
    if out_is_file && args.out.is_dir() {
        usage_error(
            &format!("{} is a directory, and a PDF is a file", args.out.display()),
            "name the PDF, such as plan.pdf",
        );
    }
    if !out_is_file && args.out.exists() && !args.out.is_dir() {
        usage_error(
            &format!("{} is not a directory", args.out.display()),
            "give a directory for the sheets; it is created when it does not exist",
        );
    }
    for (what, input) in [
        ("the drawing", Some(&args.drawing)),
        ("the CTB", args.ctb.as_ref()),
        ("the PC3", args.pc3.as_ref()),
    ] {
        if let Some(input) = input
            && same_file(input, &args.out)
        {
            usage_error(
                &format!(
                    "OUT {} is {what}, and the plot would overwrite it",
                    args.out.display()
                ),
                "give an OUT of its own",
            );
        }
    }
    if args.model && format != Format::Pvec {
        crate::operational_error(
            "model space is plotted as a PVEC only, because the core's PDF and PMTiles plots take paper layouts; use --format pvec",
        );
    }
    if !args.drawing.is_file() {
        crate::operational_error(&format!(
            "{} does not exist or is not a file",
            args.drawing.display()
        ));
    }
    #[cfg(feature = "pvec-convert")]
    {
        #[cfg(not(feature = "pdfium"))]
        if format == Format::Pmtiles {
            crate::operational_error(
                "--format pmtiles is not in this build; rebuild with the `pdfium` feature",
            );
        }
        imp::run(&args, format);
    }
    #[cfg(not(feature = "pvec-convert"))]
    {
        let _ = (args, format);
        crate::operational_error(
            "`viprs plot` is not in this build; rebuild with `--features pvec-convert`",
        );
    }
}

#[cfg(feature = "pvec-convert")]
mod imp {
    use super::*;

    use std::sync::Arc;
    use std::time::Duration;

    use libviprs::cad::CadSource;
    use libviprs::cad::plot_defaults::PlotDefaultFile;
    use libviprs::cad::plot_files::{PlotStyleTable, default_plot_style};
    use libviprs::pvec::components::StreamSelection;
    use libviprs::pvec::dwg_acadrust::AcadrustProvider;
    use libviprs::pvec::{DwgProvider, SourceUnit, ViewProfile};
    use libviprs::pvec_convert::{
        CadChoices, LayoutSetError, PdfPlot, PlotSettings, PvecConversion, PvecConvertError,
        PvecLayoutSet, SheetSkip, apply_choices, cad_manifest, check_layer_state, paper_sheets,
    };
    use libviprs::{CancelToken, EngineEvent, EngineObserver};

    use crate::operational_error;
    use crate::pipeline::{EXIT_INTERRUPTED, install_sigint};
    use crate::plot_defaults::{Source, describe, device, table, walk};
    use crate::vector::with_causes;

    /// `--progress`: the core's sheet events, as lines on stderr.
    struct Progress;

    impl EngineObserver for Progress {
        fn on_event(&self, event: EngineEvent) {
            match event {
                EngineEvent::ConversionSheetStarted {
                    index,
                    total,
                    layout,
                } => {
                    eprintln!("[{index}/{total}] {layout} started");
                }
                EngineEvent::ConversionSheetFinished {
                    index,
                    total,
                    layout,
                    status,
                    code,
                } => match code {
                    Some(code) => eprintln!("[{index}/{total}] {layout} {status} ({code})"),
                    None => eprintln!("[{index}/{total}] {layout} {status}"),
                },
                _ => {}
            }
        }
    }

    /// The error with its causes, and the core's stable code.
    fn fail_with(e: &(dyn std::error::Error + 'static), code: &str) -> ! {
        operational_error(&format!("{} ({code})", with_causes(e)))
    }

    pub(super) fn run(args: &PlotArgs, format: Format) {
        let source = CadSource::Path(&args.drawing);
        let unit = args
            .unit
            .as_deref()
            .map(|u| match u {
                "mm" => SourceUnit::Millimeters,
                "cm" => SourceUnit::Centimeters,
                "m" => SourceUnit::Meters,
                "in" => SourceUnit::Inches,
                _ => SourceUnit::Feet,
            })
            .or_else(|| {
                AcadrustProvider::open(CadSource::Path(&args.drawing))
                    .ok()
                    .map(|p| p.source_unit())
            });
        let stem = args.stem.clone().unwrap_or_else(|| {
            args.drawing
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        if format != Format::Pdf && !plain_name(&stem) {
            usage_error(
                &format!("the drawing's file stem {stem:?} is not a plain file name"),
                "give one with --stem",
            );
        }

        // The plot files first: a refusal writes nothing. Each walks the flag, the variable, the stored
        // default and the built-in on its own (libviprs-cli#162).
        let (ctb_pick, pc3_pick) = walk(args.ctb.as_deref(), args.pc3.as_deref(), args.no_defaults);
        for pick in [&ctb_pick, &pc3_pick] {
            if let Source::Env(_, path) | Source::Stored(path) = &pick.source
                && same_file(path, &args.out)
            {
                usage_error(
                    &format!(
                        "OUT {} is the default file in use ({}), and the plot would overwrite it",
                        args.out.display(),
                        path.display()
                    ),
                    "give an OUT of its own",
                );
            }
        }
        eprintln!(
            "note: plot style table: {}",
            describe(PlotDefaultFile::Ctb, &ctb_pick.source)
        );
        eprintln!(
            "note: plotter config: {}",
            describe(PlotDefaultFile::Pc3, &pc3_pick.source)
        );
        // The built-in table is the core's own for a PDF or a PMTiles plot, which notes that it used it; a
        // PVEC is handed it, so the same plot is monochrome whichever format it is written in. The built-in
        // device only fills a layout that has no paper of its own, and model space has no layout.
        let table = table(&ctb_pick).or_else(|| (format == Format::Pvec).then(default_plot_style));
        let device = if args.model && matches!(pc3_pick.source, Source::BuiltIn) {
            None
        } else {
            device(&pc3_pick)
        };
        let settings = PlotSettings {
            device,
            ..PlotSettings::default()
        };

        // The drawing's layouts and layer states, before anything is created.
        let sheets = match paper_sheets::<AcadrustProvider>(CadSource::Path(&args.drawing)) {
            Ok(sheets) => sheets,
            Err(e) => fail_with(&e, e.code()),
        };
        let paper: Vec<&str> = sheets.iter().map(|s| s.layout.as_str()).collect();
        for asked in &args.layout {
            if !paper.iter().any(|p| p.eq_ignore_ascii_case(asked)) {
                operational_error(&format!(
                    "paper layout {asked:?} not found (LAYOUT_NOT_FOUND), the paper layouts are {}",
                    paper.join(", ")
                ));
            }
        }
        let base = if args.model {
            ViewProfile::default_profile()
        } else {
            ViewProfile::print_profile("")
        };
        let profile = match &args.layer_state {
            None => base,
            Some(name) => {
                let manifest = match cad_manifest::<AcadrustProvider>(source) {
                    Ok(m) => m,
                    Err(e) => fail_with(&e, e.code()),
                };
                let choices: CadChoices = serde_json::from_value(serde_json::json!({
                    "schema": 1,
                    "mode": if args.model { "model" } else { "layouts" },
                    "layerState": name,
                }))
                .expect("the choices are well formed");
                if let Err(e) = check_layer_state(&manifest, &choices) {
                    fail_with(&e, e.code());
                }
                apply_choices(base, &manifest, &choices)
            }
        };
        // What a PDF or a PMTiles plot draws when no layout is named: every sheet, as a listing of them says.
        let wanted: Vec<String> = if args.layout.is_empty() {
            sheets
                .iter()
                .filter(|s| s.skip.is_none())
                .map(|s| s.layout.clone())
                .collect()
        } else {
            args.layout.clone()
        };
        if !args.model && wanted.is_empty() && format != Format::Pvec {
            operational_error(
                "the drawing has no paper layout to plot; `--format pvec --model` plots model space",
            );
        }

        let created_dir = format != Format::Pdf && !args.out.exists();
        if format != Format::Pdf
            && let Err(e) = std::fs::create_dir_all(&args.out)
        {
            operational_error(&format!("cannot create {}: {e}", args.out.display()));
        }
        let fail = |message: &str| -> ! {
            if created_dir {
                let _ = std::fs::remove_dir(&args.out);
            }
            operational_error(message)
        };
        let cancel: CancelToken = install_sigint();
        let observer: Option<Arc<dyn EngineObserver>> =
            args.progress.then(|| Arc::new(Progress) as _);
        let deadline = args.deadline.map(Duration::from_secs_f64);

        match format {
            Format::Pdf => {
                let mut plot = PdfPlot::new(source, &args.out)
                    .view_profile(profile)
                    .plot_settings(settings)
                    .layouts(wanted)
                    .texts(true)
                    .fills(true)
                    .cancel(cancel);
                if let Some(u) = unit {
                    plot = plot.unit(u);
                }
                if let Some(t) = table {
                    plot = plot.plot_style_table(t);
                }
                if let Some(o) = observer {
                    plot = plot.observer(o);
                }
                if let Some(d) = deadline {
                    plot = plot.deadline(d);
                }
                match plot.convert() {
                    Ok(out) => {
                        eprintln!("pdf  written  {} ({} pages)", out.file.display(), out.pages);
                        for note in &out.notes {
                            eprintln!("note: {note}");
                        }
                    }
                    Err(PvecConvertError::Cancelled) => std::process::exit(EXIT_INTERRUPTED),
                    Err(e) => fail_with(&e, e.code()),
                }
            }
            Format::Pmtiles => {
                if let Err(message) = pmtiles(
                    args, source, unit, &stem, profile, settings, table, wanted, cancel, observer,
                    deadline,
                ) {
                    fail(&message);
                }
            }
            Format::Pvec if args.model => {
                let dest: PathBuf = args.out.join(format!("{stem}.pvec"));
                let mut c = PvecConversion::new(source, &dest)
                    .view_profile(profile)
                    .plot_settings(settings)
                    .components(StreamSelection::ALL)
                    .annotations(true)
                    .cancel(cancel);
                if let Some(u) = unit {
                    c = c.unit(u);
                }
                if let Some(t) = table {
                    c = c.plot_style_table(t);
                }
                if let Some(o) = observer {
                    c = c.observer(o);
                }
                if let Some(d) = deadline {
                    c = c.deadline(d);
                }
                match c.convert() {
                    Ok(out) => {
                        let name = dest
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        eprintln!("model  written  {name} ({} entities)", out.written);
                    }
                    Err(PvecConvertError::Cancelled) => std::process::exit(EXIT_INTERRUPTED),
                    Err(e) => fail(&format!("{} ({})", with_causes(&e), e.code())),
                }
            }
            Format::Pvec => {
                let layer_state = args.layer_state.is_some();
                let mut set = PvecLayoutSet::new(source, &args.out)
                    .stem(stem)
                    .sheets(move |c| {
                        let mut c = c
                            .plot_settings(settings.clone())
                            .components(StreamSelection::ALL)
                            .annotations(true)
                            .cancel(cancel.clone());
                        if layer_state {
                            c = c.view_profile(profile.clone());
                        }
                        if let Some(u) = unit {
                            c = c.unit(u);
                        }
                        if let Some(t) = &table {
                            c = c.plot_style_table(t.clone());
                        }
                        if let Some(o) = &observer {
                            c = c.observer(o.clone());
                        }
                        c
                    });
                if !args.layout.is_empty() {
                    set = set.layouts(args.layout.iter().cloned());
                }
                if let Some(d) = deadline {
                    set = set.deadline(d);
                }
                let out = match set.convert() {
                    Ok(out) => out,
                    Err(LayoutSetError::Cancelled) => std::process::exit(EXIT_INTERRUPTED),
                    Err(e) => fail(&format!("{} ({})", with_causes(&e), e.code())),
                };
                for s in &out.sheets {
                    let name = s
                        .file
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let (status, detail) = match &s.outcome {
                        Ok(o) => ("written", format!("({} entities)", o.written)),
                        Err(SheetSkip::Failed(e)) => {
                            ("failed", format!("{} ({})", with_causes(e), e.code()))
                        }
                        Err(skip) => (
                            "empty",
                            format!("nothing to print, no file ({})", skip.code()),
                        ),
                    };
                    eprintln!("{:>3} {}  {status}  {name} {detail}", s.tab_order, s.layout);
                }
                let failed = out.failed().count();
                if failed > 0 {
                    operational_error(&format!("{failed} of {} sheets failed", out.sheets.len()));
                }
                if out.sheets.is_empty() {
                    fail(
                        "the drawing has no paper layout, so no sheet was written; `--model` plots model space",
                    );
                }
                if out.written().count() == 0 {
                    operational_error("every layout was empty, so no sheet was written");
                }
            }
        }
    }

    /// `--format pmtiles`: one archive per sheet through the core, and the index the core leaves to the caller.
    #[cfg(feature = "pdfium")]
    #[allow(clippy::too_many_arguments)]
    fn pmtiles(
        args: &PlotArgs,
        source: CadSource<'_>,
        unit: Option<SourceUnit>,
        stem: &str,
        profile: ViewProfile,
        settings: PlotSettings,
        table: Option<PlotStyleTable>,
        wanted: Vec<String>,
        cancel: CancelToken,
        observer: Option<Arc<dyn EngineObserver>>,
        deadline: Option<Duration>,
    ) -> Result<(), String> {
        use libviprs::pvec_convert::{PmTilesPlot, PmTilesPlotError};
        let index = args.out.join(format!("{stem}.sheets.json"));
        // An index from an earlier run would say this one finished, so it goes before any archive.
        if let Err(e) = std::fs::remove_file(&index)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(format!("cannot remove {}: {e}", index.display()));
        }
        let mut plot = PmTilesPlot::new(source, &args.out)
            .stem(stem)
            .view_profile(profile)
            .plot_settings(settings)
            .layouts(wanted)
            .cancel(cancel);
        if let Some(dpi) = args.dpi {
            plot = plot.dpi(dpi);
        }
        if let Some(u) = unit {
            plot = plot.unit(u);
        }
        if let Some(t) = table {
            plot = plot.plot_style_table(t);
        }
        if let Some(o) = observer {
            plot = plot.observer(o);
        }
        if let Some(d) = deadline {
            plot = plot.deadline(d);
        }
        let out = match plot.convert_with::<AcadrustProvider>() {
            Ok(out) => out,
            Err(PmTilesPlotError::Cancelled) => std::process::exit(EXIT_INTERRUPTED),
            Err(e) => return Err(format!("{} ({})", with_causes(&e), e.code())),
        };
        let mut rows = Vec::new();
        for (n, s) in out.sheets.iter().enumerate() {
            let name = s
                .file
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            eprintln!(
                "{:>3} {}  written  {name} ({}x{} px, zoom {} to {}, {} tiles)",
                n + 1,
                s.layout,
                s.width_px,
                s.height_px,
                s.min_zoom,
                s.max_zoom,
                s.tiles
            );
            rows.push(serde_json::json!({
                "layout": s.layout,
                "file": name,
                "status": "written",
                "width_px": s.width_px,
                "height_px": s.height_px,
                "min_zoom": s.min_zoom,
                "max_zoom": s.max_zoom,
                "tiles": s.tiles,
            }));
        }
        for note in &out.notes {
            eprintln!("note: {note}");
        }
        // Written beside the archives under a name of its own and renamed into place, so an index that
        // is there is a whole one.
        let partial = args.out.join(format!("{stem}.sheets.json.partial"));
        let body = serde_json::json!({
            "schema_version": 1,
            "source": args.drawing.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            "stem": stem,
            "sheets": rows,
        });
        let written = serde_json::to_vec_pretty(&body)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&partial, bytes))
            .and_then(|()| std::fs::rename(&partial, &index));
        if let Err(e) = written {
            let _ = std::fs::remove_file(&partial);
            return Err(format!("cannot write {}: {e}", index.display()));
        }
        Ok(())
    }

    #[cfg(not(feature = "pdfium"))]
    #[allow(clippy::too_many_arguments)]
    fn pmtiles(
        _: &PlotArgs,
        _: CadSource<'_>,
        _: Option<SourceUnit>,
        _: &str,
        _: ViewProfile,
        _: PlotSettings,
        _: Option<PlotStyleTable>,
        _: Vec<String>,
        _: CancelToken,
        _: Option<Arc<dyn EngineObserver>>,
        _: Option<Duration>,
    ) -> Result<(), String> {
        Err("--format pmtiles is not in this build; rebuild with the `pdfium` feature".to_owned())
    }
}
