//! `viprs pvec convert` (libviprs#1566, slice C of libviprs#1557): a DWG to `.pvec` files.
//!
//! ```text
//! viprs pvec convert <drawing.dwg> <out-dir>                 every paper layout, one .pvec each
//! viprs pvec convert <drawing.dwg> <out-dir> --layout A-101  only that layout (repeatable)
//! viprs pvec convert <drawing.dwg> <out-dir> --model         model space, one .pvec
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
    use libviprs::pvec::SourceUnit;
    use libviprs::pvec_convert::{
        LayoutSetError, PvecConversion, PvecConvertError, PvecLayoutSet, SheetSkip,
    };

    use crate::operational_error;
    use crate::pipeline::{EXIT_INTERRUPTED, install_sigint};
    use crate::vector::with_causes;

    pub(super) fn run(args: &ConvertArgs) {
        let unit = args.unit.as_deref().map(|u| match u {
            "mm" => SourceUnit::Millimeters,
            "cm" => SourceUnit::Centimeters,
            "m" => SourceUnit::Meters,
            "in" => SourceUnit::Inches,
            _ => SourceUnit::Feet,
        });
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
            let mut c = PvecConversion::new(CadSource::Path(&args.drawing), &dest).cancel(cancel);
            if let Some(u) = unit {
                c = c.unit(u);
            }
            match c.convert() {
                Ok(out) => {
                    eprintln!("model  written  {} ({} entities)", name(&dest), out.written);
                }
                Err(PvecConvertError::Cancelled) => std::process::exit(EXIT_INTERRUPTED),
                Err(e) => fail(&with_causes(&e)),
            }
            return;
        }

        let mut set = PvecLayoutSet::new(CadSource::Path(&args.drawing), &args.out_dir)
            .stem(stem)
            .sheets(move |c| {
                let c = c.cancel(cancel.clone());
                match unit {
                    Some(u) => c.unit(u),
                    None => c,
                }
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
                Err(SheetSkip::Failed(e)) => ("failed", with_causes(e)),
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
        if written == 0 {
            operational_error(if out.sheets.is_empty() {
                "the drawing has no paper layout, so no sheet was written; `--model` converts model space"
            } else {
                "every layout converted was empty, so no sheet was written"
            });
        }
    }

    fn name(p: &Path) -> String {
        p.file_name().map_or_else(
            || p.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    }
}
