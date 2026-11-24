//! `viprs pvec convert PAGE.pdf OUT_DIR` (libviprs-cli#151): one page of a PDF into `<stem>.pvec` and
//! `<stem>.pvec.report.json`, through the core's `PdfConversion`. The page is `--page` (1 by default)
//! and the image limits are the flags of `knobs::PdfArgs`.
//!
//! The core publishes the artifact and its report in one step, so a run that cannot be called a
//! success after the fact (a page whose images need the JBIG2 decoder this build left out) would
//! leave both behind. They are written under a staging name beside the destination and moved into
//! place only when the run is good; the staging files go again when it is not.
//!
//! Exit codes are the command's: 0 when the page was written, 1 when it could not be (the page is
//! not in the document, it has no path, a limit was passed, the PDF could not be read, or a JBIG2
//! stream needs a feature this build lacks), 130 for Ctrl-C.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use libviprs::cad::CadSource;
use libviprs::pvec::pdf_image::PdfImageOptions;
use libviprs::pvec::pdf_vector::PdfVectorLimits;
use libviprs::pvec_convert::{PdfConversion, PdfConvertError};

use super::ConvertArgs;
use crate::operational_error;
use crate::pipeline::{EXIT_INTERRUPTED, install_sigint};
use crate::vector::with_causes;

/// The report that sits beside an artifact: its path with `.report.json` appended.
fn report_of(artifact: &Path) -> PathBuf {
    let mut name: OsString = artifact.as_os_str().to_owned();
    name.push(".report.json");
    PathBuf::from(name)
}

pub(super) fn run(args: &ConvertArgs) {
    let page = args.pdf.page.unwrap_or(1);
    // The limits that stop the page being read, and the options that decide what is baked. A page over
    // the image count or the image bytes is refused on the way in, so both are set from the one flag.
    let mut limits = PdfVectorLimits::default();
    let mut images = PdfImageOptions::default();
    if let Some(n) = args.pdf.max_images {
        limits.max_images = n;
        images.max_images = n;
    }
    if let Some(n) = args.pdf.max_image_bytes {
        limits.max_image_bytes = n;
        images.max_image_bytes = n;
    }
    if let Some(n) = args.pdf.max_image_stream_bytes {
        limits.max_image_stream_bytes = n;
    }
    if let Some(n) = args.pdf.max_image_pixels {
        images.max_image_pixels = n;
    }
    if let Some(n) = args.pdf.max_pixels_per_image {
        images.max_pixels = n;
    }

    // A directory this run made goes again when the run writes nothing into it.
    let created = !args.out_dir.exists();
    if let Err(e) = std::fs::create_dir_all(&args.out_dir) {
        operational_error(&format!("cannot create {}: {e}", args.out_dir.display()));
    }
    let stem = args.stem.clone().unwrap_or_else(|| {
        args.drawing
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    });
    let dest = args.out_dir.join(format!("{stem}.pvec"));
    let staged = args
        .out_dir
        .join(format!(".{stem}.{}.pvec", std::process::id()));
    let drop_staged = || {
        let _ = std::fs::remove_file(&staged);
        let _ = std::fs::remove_file(report_of(&staged));
    };
    let fail = |message: &str| -> ! {
        drop_staged();
        if created {
            let _ = std::fs::remove_dir(&args.out_dir);
        }
        operational_error(message)
    };

    let converted = PdfConversion::new(CadSource::Path(&args.drawing), &staged)
        .page(page as usize)
        .limits(limits)
        .image_options(images)
        .cancel(install_sigint())
        .convert();
    let out = match converted {
        Ok(out) => out,
        Err(PdfConvertError::Cancelled) => {
            drop_staged();
            std::process::exit(EXIT_INTERRUPTED)
        }
        Err(e) => fail(&with_causes(&e)),
    };

    let refused = out
        .report
        .pdf
        .as_ref()
        .map(|p| p.images_refused.clone())
        .unwrap_or_default();
    if cfg!(not(feature = "jbig2"))
        && refused
            .keys()
            .any(|k| k.starts_with("filter-not-enabled:JBIG2"))
    {
        fail(&format!(
            "page {page} has JBIG2 image streams and this build has no decoder for them; rebuild with `--features jbig2`"
        ));
    }
    for (from, to) in [
        (&staged, dest.clone()),
        (&report_of(&staged), report_of(&dest)),
    ] {
        if let Err(e) = std::fs::rename(from, &to) {
            fail(&format!("cannot write {}: {e}", to.display()));
        }
    }
    let name = dest.file_name().map_or_else(
        || dest.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    eprintln!(
        "page {page}  written  {name} ({} entities, {} rasters)",
        out.written, out.rasters
    );
    if !refused.is_empty() {
        let list: Vec<String> = refused.iter().map(|(k, n)| format!("{k} x{n}")).collect();
        eprintln!("images left out: {}", list.join(", "));
    }
}
