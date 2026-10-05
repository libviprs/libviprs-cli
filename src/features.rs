//! What this binary was built with (`viprs features`), and the refusal a
//! format gets when its feature was left out (libviprs-cli#64).
//!
//! A user who hits a decode error has no way to tell "this file is broken"
//! from "this viprs has no decoder for it" unless the binary says so. Both
//! halves live here so the list `viprs features` prints and the feature names
//! the refusals quote come from one place.

use std::fmt;

use libviprs::source::SourceError;
use libviprs::{AvifError, Jp2kError, JxlError};

/// Every cargo feature of this crate that changes what the binary can do,
/// in the order `viprs features` prints them (alphabetical), each paired with
/// whether this build has it.
///
/// `full` is not here: it is a bundle of the others, so a `full` build lists
/// them rather than itself. `default` is not here for the same reason.
const FEATURES: &[(&str, bool)] = &[
    ("avif", cfg!(feature = "avif")),
    ("jp2k", cfg!(feature = "jp2k")),
    ("jxl", cfg!(feature = "jxl")),
    ("object-store-sink", cfg!(feature = "object-store-sink")),
    ("packfile", cfg!(feature = "packfile")),
    ("pdfium", cfg!(feature = "pdfium")),
    ("pdfium-static", cfg!(feature = "pdfium-static")),
    ("s3", cfg!(feature = "s3")),
    ("svg", cfg!(feature = "svg")),
    ("tracing", cfg!(feature = "tracing")),
];

/// The features compiled into this binary, in [`FEATURES`] order.
pub fn compiled() -> Vec<&'static str> {
    FEATURES
        .iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| *name)
        .collect()
}

/// Arguments of `viprs features`.
#[derive(clap::Parser)]
pub struct FeaturesArgs {
    /// Print `{"v": 1, "features": [...]}` instead of one name per line.
    #[arg(long)]
    pub json: bool,
}

/// `viprs features [--json]`: one feature per line, or
/// `{"v": 1, "features": [...]}` with `--json`. `v` is the format's version,
/// so a script can tell a shape it knows from a later one. Always exits 0; an
/// empty list is a valid answer for a `--no-default-features` build.
pub fn run(args: FeaturesArgs) {
    let features = compiled();
    if args.json {
        println!("{}", serde_json::json!({ "v": 1, "features": features }));
    } else {
        for name in features {
            println!("{name}");
        }
    }
}

/// A decode that failed only because this build left out the feature the
/// format needs.
///
/// Its message names the feature twice on purpose: once as the thing that is
/// missing and once as the flag to rebuild with, because the second is what
/// the person reading it will type next.
#[derive(Debug)]
pub struct MissingFeature {
    /// The cargo feature, as `--features` spells it.
    pub feature: &'static str,
    /// The format, as a person would call it.
    pub format: &'static str,
}

impl fmt::Display for MissingFeature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "this viprs was built without the `{feature}` feature, so it cannot decode {format}; \
             rebuild it with `--features {feature}` (or `--features full`), and run \
             `viprs features` to see what this build has",
            feature = self.feature,
            format = self.format,
        )
    }
}

impl std::error::Error for MissingFeature {}

/// The save-side twin of [`MissingFeature`]: an encode that failed only
/// because this build left out the feature the format needs (libviprs-cli#65).
#[derive(Debug)]
pub struct MissingEncoder {
    /// The cargo feature, as `--features` spells it.
    pub feature: &'static str,
    /// The format, as a person would call it.
    pub format: &'static str,
}

impl fmt::Display for MissingEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "this viprs was built without the `{feature}` feature, so it cannot encode {format}; \
             rebuild it with `--features {feature}` (or `--features full`), and run \
             `viprs features` to see what this build has",
            feature = self.feature,
            format = self.format,
        )
    }
}

impl std::error::Error for MissingEncoder {}

/// The [`MissingFeature`] behind a core decode error, if that is what it is.
///
/// Matched on the core's typed `FeatureNotEnabled` variants. Each of them is
/// declared in every build of the core, so this compiles the same with the
/// feature on or off; with it on, the arm is simply never taken. SVG is the
/// exception: the core has no typed variant for it, only an `Unsupported` I/O
/// error that names the feature, so in a build without `svg` that kind plus
/// that name is what's matched.
pub fn missing_feature(err: &SourceError) -> Option<MissingFeature> {
    let (feature, format) = match err {
        SourceError::Avif(AvifError::FeatureNotEnabled) => ("avif", "AVIF"),
        SourceError::Jxl(JxlError::FeatureNotEnabled) => ("jxl", "JPEG XL"),
        SourceError::Jp2k(Jp2kError::FeatureNotEnabled) => ("jp2k", "JPEG 2000"),
        // Without `svg` the core's rasteriser has one answer, an `Unsupported`
        // I/O error naming the feature, whether the SVG came by extension or
        // through the core's own content sniff (libviprs#1170). With the
        // feature on, the arm is not compiled, so an `Unsupported` from the
        // renderer stays what it is.
        #[cfg(not(feature = "svg"))]
        SourceError::Io(e)
            if e.kind() == std::io::ErrorKind::Unsupported
                && e.to_string().contains("`svg` feature") =>
        {
            ("svg", "SVG")
        }
        _ => return None,
    };
    Some(MissingFeature { feature, format })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listing_is_in_alphabetical_order_and_has_no_duplicates() {
        let names: Vec<&str> = FEATURES.iter().map(|(n, _)| *n).collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(names, sorted);
    }

    /// Every feature in the manifest that is not a bundle is in the listing,
    /// and nothing is listed that the manifest does not declare. Read from the
    /// manifest so a feature added there without a row here fails here.
    #[test]
    fn the_listing_covers_exactly_the_manifests_features() {
        let manifest = include_str!("../Cargo.toml");
        let mut declared = Vec::new();
        let mut inside = false;
        for line in manifest.lines() {
            let t = line.trim();
            if t.starts_with('[') && t.ends_with(']') {
                inside = t == "[features]";
                continue;
            }
            if !inside || t.is_empty() || t.starts_with('#') || t.starts_with('"') {
                continue;
            }
            if let Some((name, _)) = t.split_once('=') {
                let name = name.trim();
                if name != "default" && name != "full" {
                    declared.push(name);
                }
            }
        }
        declared.sort_unstable();
        let listed: Vec<&str> = FEATURES.iter().map(|(n, _)| *n).collect();
        assert_eq!(listed, declared);
    }

    #[test]
    fn the_refusal_names_the_feature_and_the_flag() {
        let msg = MissingFeature {
            feature: "jxl",
            format: "JPEG XL",
        }
        .to_string();
        assert!(msg.contains("`jxl`"), "{msg}");
        assert!(msg.contains("--features jxl"), "{msg}");
        assert!(!msg.to_ascii_lowercase().contains("unsupported"), "{msg}");
    }

    #[test]
    fn a_feature_off_core_error_maps_to_its_feature() {
        let cases = [
            (SourceError::from(AvifError::FeatureNotEnabled), "avif"),
            (SourceError::from(JxlError::FeatureNotEnabled), "jxl"),
            (SourceError::from(Jp2kError::FeatureNotEnabled), "jp2k"),
        ];
        for (err, want) in cases {
            assert_eq!(missing_feature(&err).map(|m| m.feature), Some(want));
        }
        let io = SourceError::from(std::io::Error::other("not a feature"));
        assert!(missing_feature(&io).is_none());
    }
}
