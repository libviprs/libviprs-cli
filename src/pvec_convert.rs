//! `viprs pvec convert` (libviprs#1566, slice C of libviprs#1557): a DWG to `.pvec` files.
//!
//! ```text
//! viprs pvec convert <drawing.dwg> <out-dir>                 every paper layout, one .pvec each
//! viprs pvec convert <drawing.dwg> <out-dir> --layout A-101  only that layout (repeatable)
//! viprs pvec convert <drawing.dwg> <out-dir> --model         model space, one .pvec
//! viprs pvec convert <drawing.dwg> <out-dir> --regions       also rooms and corridors
//! viprs pvec convert <drawing.dwg> <out-dir> --geometry-only no layer, line style or provenance
//! ```
//!
//! A thin call onto `libviprs::pvec_convert`: `PvecLayoutSet` for the sheets, `PvecConversion` for
//! model space. A sheet is `<stem>.<NN>-<slug>.pvec` with its report beside it and the set index
//! `<stem>.sheets.json` written last; model space is `<stem>.pvec` and its report. This file
//! parses arguments, prints one line per sheet and picks the exit code.
//!
//! # Exit codes
//!
//! 0 when every sheet asked for was written or skipped as empty and at least one was written; 1 when
//! a sheet failed, when no sheet was written (a drawing with no paper layout, or only empty ones;
//! model space is `--model`, never a fallback), or the drawing did not convert; 2 for a usage
//! mistake; 130 when Ctrl-C stopped it, with the sheets already published left in place and no index.

use std::path::PathBuf;

use clap::Args;

use crate::usage_error;

/// `viprs pvec convert`.
#[derive(Args)]
pub(crate) struct ConvertArgs {
    /// The drawing (a DWG).
    drawing: PathBuf,

    /// The directory the files go into. It is created when it does not exist.
    out_dir: PathBuf,

    /// Convert only this paper layout (repeatable). The name is matched with
    /// ASCII case ignored; a name the drawing does not have converts nothing
    /// and exits 1.
    #[arg(long, value_name = "NAME", conflicts_with = "model")]
    layout: Vec<String>,

    /// Convert model space to one `<stem>.pvec` instead of the paper layouts.
    #[arg(long)]
    model: bool,

    /// The file stem, instead of the drawing's own file stem.
    #[arg(long)]
    stem: Option<String>,

    /// The drawing's unit when its header declares none (`mm`, `cm`, `m`,
    /// `in`, `ft`). A sheet whose plot settings name inches or millimetres is
    /// in that unit whatever this says.
    #[arg(long, value_parser = ["mm", "cm", "m", "in", "ft"])]
    unit: Option<String>,

    /// Write the geometry only. Without this the layer, line style and
    /// provenance streams are written beside it, as the library does when it
    /// is asked for every component.
    #[arg(long)]
    geometry_only: bool,

    /// Also derive rooms and corridors from the wall linework and write them
    /// as the class stream. They are a guess from the walls and the TEXT, so
    /// they are off unless asked for.
    #[arg(long)]
    regions: bool,
}

pub(crate) fn run(args: ConvertArgs) {
    if args.out_dir.exists() && !args.out_dir.is_dir() {
        usage_error(
            &format!("{} is not a directory", args.out_dir.display()),
            "give a directory for the .pvec files; it is created when it does not exist",
        );
    }
    if !args.drawing.is_file() {
        // An input that cannot be read is an operational failure, as for `vector`.
        crate::operational_error(&format!(
            "{} does not exist or is not a file",
            args.drawing.display()
        ));
    }
    if let Some(stem) = &args.stem
        && (stem.is_empty() || stem.contains(['/', '\\']) || stem == "." || stem == "..")
    {
        usage_error(
            &format!("--stem {stem:?} is not a plain file name"),
            "give a name with no path separator, such as `--stem plan`",
        );
    }
    #[cfg(feature = "pvec-convert")]
    imp::run(&args);
    #[cfg(not(feature = "pvec-convert"))]
    {
        let _ = args;
        crate::operational_error(
            "`viprs pvec convert` is not in this build; rebuild with `--features pvec-convert`",
        );
    }
}

#[cfg(feature = "pvec-convert")]
mod imp {
    use super::*;

    use std::path::Path;

    use libviprs::cad::CadSource;
    use libviprs::pvec::DwgProvider;
    use libviprs::pvec::SourceUnit;
    use libviprs::pvec::components::StreamSelection;
    use libviprs::pvec::dwg_acadrust::AcadrustProvider;
    use libviprs::pvec::regions::RegionConfig;
    use libviprs::pvec::writer::WriteError;
    use libviprs::pvec_convert::{
        LayoutSetError, PvecConversion, PvecConvertError, PvecLayoutSet, SheetSkip,
    };

    use crate::operational_error;
    use crate::pipeline::{EXIT_INTERRUPTED, install_sigint};
    use crate::vector::with_causes;

    /// How many of `unit` make a metre, for the length knobs of the regions step, which are in
    /// drawing units. A unit that is not a plain length (or none) counts as metres.
    pub(super) fn units_per_metre(unit: SourceUnit) -> f64 {
        match unit {
            SourceUnit::Millimeters => 1000.0,
            SourceUnit::Centimeters => 100.0,
            SourceUnit::Inches => 1.0 / 0.0254,
            SourceUnit::Feet => 1.0 / 0.3048,
            SourceUnit::Yards => 1.0 / 0.9144,
            SourceUnit::Kilometers => 0.001,
            _ => 1.0,
        }
    }

    /// The unit, the component streams and the regions step, the same for model space and a sheet.
    fn configure<'a>(
        c: PvecConversion<'a>,
        unit: Option<SourceUnit>,
        geometry_only: bool,
        regions: bool,
        per_metre: f64,
    ) -> PvecConversion<'a> {
        let c = c.components(if geometry_only {
            StreamSelection::NONE
        } else {
            StreamSelection::ALL
        });
        let c = c.regions(regions.then_some(RegionConfig {
            // 0.5 m and 2.5 m, in the drawing's own unit.
            gap_tolerance: 0.5 * per_metre,
            corridor_max_width: 2.5 * per_metre,
            corridor_min_aspect: 3.0,
            max_faces: 1000,
            keep_unlabelled: false,
        }));
        match unit {
            Some(u) => c.unit(u),
            None => c,
        }
    }

    /// The error with its causes, and the flag to pass when the drawing declares no unit and none
    /// was given.
    fn explain(e: &(dyn std::error::Error + 'static), unit_given: bool) -> String {
        let mut text = with_causes(e);
        let mut cause = Some(e);
        while let Some(c) = cause {
            if !unit_given
                && matches!(
                    c.downcast_ref::<WriteError>(),
                    Some(WriteError::UnsupportedUnit(SourceUnit::Unitless))
                )
            {
                text.push_str("; the drawing declares no unit, pass --unit mm, cm, m, in or ft");
                break;
            }
            cause = c.source();
        }
        text
    }

    pub(super) fn run(args: &ConvertArgs) {
        let unit = args.unit.as_deref().map(|u| match u {
            "mm" => SourceUnit::Millimeters,
            "cm" => SourceUnit::Centimeters,
            "m" => SourceUnit::Meters,
            "in" => SourceUnit::Inches,
            _ => SourceUnit::Feet,
        });
        // The regions step wants lengths in the drawing's unit: the one given, else the header's.
        let per_metre = if args.regions {
            let effective = unit.or_else(|| {
                AcadrustProvider::open(CadSource::Path(&args.drawing))
                    .ok()
                    .map(|p| p.source_unit())
            });
            effective.map_or(1.0, units_per_metre)
        } else {
            1.0
        };
        // A directory this run made goes again when the run writes nothing into it.
        let created = !args.out_dir.exists();
        if let Err(e) = std::fs::create_dir_all(&args.out_dir) {
            operational_error(&format!("cannot create {}: {e}", args.out_dir.display()));
        }
        let fail = |message: &str| -> ! {
            if created {
                let _ = std::fs::remove_dir(&args.out_dir);
            }
            operational_error(message)
        };
        let cancel = install_sigint();
        let stem = args.stem.clone().unwrap_or_else(|| {
            args.drawing
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        });

        if args.model {
            let dest: PathBuf = args.out_dir.join(format!("{stem}.pvec"));
            let c = PvecConversion::new(CadSource::Path(&args.drawing), &dest).cancel(cancel);
            match configure(c, unit, args.geometry_only, args.regions, per_metre).convert() {
                Ok(out) => {
                    eprintln!("model  written  {} ({} entities)", name(&dest), out.written);
                }
                Err(PvecConvertError::Cancelled) => std::process::exit(EXIT_INTERRUPTED),
                Err(e) => fail(&explain(&e, unit.is_some())),
            }
            return;
        }

        let (geometry_only, regions) = (args.geometry_only, args.regions);
        let mut set = PvecLayoutSet::new(CadSource::Path(&args.drawing), &args.out_dir)
            .stem(stem)
            .sheets(move |c| {
                configure(
                    c.cancel(cancel.clone()),
                    unit,
                    geometry_only,
                    regions,
                    per_metre,
                )
            });
        if !args.layout.is_empty() {
            set = set.layouts(args.layout.iter().cloned());
        }
        let out = match set.convert() {
            Ok(out) => out,
            Err(LayoutSetError::Cancelled) => std::process::exit(EXIT_INTERRUPTED),
            Err(e) => fail(&with_causes(&e)),
        };
        for s in &out.sheets {
            let (status, detail) = match &s.outcome {
                Ok(o) => ("written", format!("({} entities)", o.written)),
                Err(SheetSkip::Empty) => ("empty", "nothing to print, no file".to_owned()),
                Err(SheetSkip::EmptyAfterView) => (
                    "empty",
                    "nothing drawn under the print profile, no file".to_owned(),
                ),
                Err(SheetSkip::Failed(e)) => ("failed", explain(e, unit.is_some())),
                Err(_) => ("skipped", String::new()),
            };
            eprintln!(
                "{:>3} {}  {status}  {} {detail}",
                s.tab_order,
                s.layout,
                name(&s.file)
            );
        }
        if let Some(index) = &out.index {
            eprintln!("index  {}", name(index));
        }
        let failed = out.failed().count();
        let written = out.written().count();
        if failed > 0 {
            operational_error(&format!("{failed} of {} sheets failed", out.sheets.len()));
        }
        if out.sheets.is_empty() {
            // Nothing was written, so a directory this run made goes again.
            fail(
                "the drawing has no paper layout, so no sheet was written; `--model` converts model space",
            );
        }
        if written == 0 {
            operational_error("every layout converted was empty, so no sheet was written");
        }
    }

    fn name(p: &Path) -> String {
        p.file_name().map_or_else(
            || p.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    }
}

#[cfg(all(test, feature = "pvec-convert"))]
mod tests {
    use super::imp::units_per_metre;
    use libviprs::pvec::SourceUnit;

    /// libviprs#1608: the 0.5 m gap and 2.5 m corridor become 500 and 2500 in a millimetre drawing.
    #[test]
    fn issue_1608_region_lengths_follow_the_drawing_unit() {
        assert_eq!(0.5 * units_per_metre(SourceUnit::Millimeters), 500.0);
        assert_eq!(2.5 * units_per_metre(SourceUnit::Millimeters), 2500.0);
        assert_eq!(0.5 * units_per_metre(SourceUnit::Centimeters), 50.0);
        assert_eq!(units_per_metre(SourceUnit::Meters), 1.0);
        assert!((units_per_metre(SourceUnit::Feet) - 3.280_839_9).abs() < 1e-6);
        assert!((units_per_metre(SourceUnit::Inches) - 39.370_078_7).abs() < 1e-6);
        assert_eq!(units_per_metre(SourceUnit::Unitless), 1.0);
    }
}
