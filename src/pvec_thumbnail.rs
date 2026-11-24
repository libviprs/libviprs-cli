//! `viprs pvec thumbnail FILE OUT.png [--width N]` (libviprs-cli#150, libviprs#1629): a PNG of a
//! published `.pvec`, strokes only (no text, no fills), black on opaque white.
//!
//! A thin call onto `libviprs::pvec::thumbnail`. The picture is the artifact's declared bounds, so
//! its height follows their aspect. The core caps neither side, so the cap lives here: the width
//! is 1 to `MAX_WIDTH` and a derived height past `MAX_HEIGHT` is refused before anything is drawn.
//!
//! # Exit codes
//!
//! 0 when the PNG was written; 1 when the file is missing or is not a readable `.pvec`, when the
//! height would pass the cap, when OUT cannot be written, and in a build without the
//! `pvec-raster-thumbnail` feature (which names it); 2 for a usage mistake, a zero width or one
//! past the cap included, and in every build, before the file is opened.

use std::path::PathBuf;

use clap::Args;

use crate::pvec_convert::MAX_THUMBNAIL_WIDTH;

/// The width when `--width` is not given.
const DEFAULT_WIDTH: u32 = 512;

/// The tallest picture, in pixels. A long thin sheet at the widest width would otherwise ask for
/// a pixmap of gigabytes.
#[cfg_attr(not(feature = "pvec-raster-thumbnail"), allow(dead_code))]
const MAX_HEIGHT: u32 = 16384;

/// `viprs pvec thumbnail`.
#[derive(Args)]
pub(crate) struct ThumbnailArgs {
    /// The `.pvec` to draw.
    file: PathBuf,

    /// The PNG to write.
    output: PathBuf,

    /// The picture's width in pixels, 1 to 4096. The height follows the artifact's bounds and
    /// has to stay within 16384. It draws strokes only: no text, no fills, one pixel wide, black on white.
    #[arg(
        long,
        value_name = "PIXELS",
        default_value_t = DEFAULT_WIDTH,
        value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_THUMBNAIL_WIDTH))
    )]
    width: u32,
}

pub(crate) fn run(args: ThumbnailArgs) {
    if !args.file.is_file() {
        crate::operational_error(&format!(
            "{} does not exist or is not a file",
            args.file.display()
        ));
    }
    #[cfg(feature = "pvec-raster-thumbnail")]
    imp::run(&args);
    #[cfg(not(feature = "pvec-raster-thumbnail"))]
    {
        let _ = args;
        crate::operational_error(
            "`viprs pvec thumbnail` is not in this build; rebuild with `--features pvec-raster-thumbnail`",
        );
    }
}

#[cfg(feature = "pvec-raster-thumbnail")]
mod imp {
    use super::*;

    use libviprs::pvec::publish::open_published;
    use libviprs::pvec::thumbnail::thumbnail;

    use crate::operational_error;
    use crate::vector::with_causes;

    pub(super) fn run(args: &ThumbnailArgs) {
        let bounds = match open_published(&args.file) {
            Ok(reader) => reader.manifest().bounds,
            Err(e) => operational_error(&format!(
                "{} is not a readable .pvec: {}",
                args.file.display(),
                with_causes(&e)
            )),
        };
        // The height the core derives for a picture this wide: the bounds' aspect, at least one
        // pixel, and a square for bounds with no extent.
        let (w, h) = (bounds.max[0] - bounds.min[0], bounds.max[1] - bounds.min[1]);
        let (w, h) = if w > 0.0 && h > 0.0 {
            (w, h)
        } else {
            (1.0, 1.0)
        };
        let height = (f64::from(args.width) * h / w).round().max(1.0);
        if height > f64::from(MAX_HEIGHT) {
            operational_error(&format!(
                "a picture {} pixels wide would be {height} pixels tall, past the {MAX_HEIGHT} this command draws; give a smaller --width",
                args.width
            ));
        }
        let png = match thumbnail(&args.file, args.width) {
            Ok(png) => png,
            Err(e) => operational_error(&with_causes(&e)),
        };
        if let Err(e) = std::fs::write(&args.output, &png) {
            operational_error(&format!("cannot write {}: {e}", args.output.display()));
        }
        eprintln!(
            "wrote {} ({} x {height})",
            args.output.display(),
            args.width
        );
    }
}
