//! Histogram op family — a per-family Wave-2 lane (`CLI_CONTRACT.md` §3/§6,
//! `OP_MAP.md` histogram section).
//!
//! The core `src/histogram.rs` exposes 29 `pub fn` that fold to fifteen base
//! ops; `case` is EXCLUDED (vips `case` needs an image-array of cases the core
//! has no CLI-shaped mirror for) and `hist_find_band` folds into
//! `hist_find --band N`, leaving **thirteen** `viprs` subcommands here. Every
//! command mirrors the vips 8.18.4 positional order, flag names, and value
//! bounds exactly (verified against `vips <op>`):
//!
//! | command | vips | shape | oracle | notes |
//! |---|---|---|---|---|
//! | `hist_find IN OUT --band`        | `hist_find`         | S1 | EXACT | uint counts; core writes ushort (differential casts the vips ref) and keeps the full 256-bin range (vips trims trailing-zero bins to width max+1) |
//! | `hist_find_indexed IN INDEX OUT` | `hist_find_indexed` | S2 | EXACT | per-index sample sums (default `sum` combine) |
//! | `hist_find_ndim IN OUT --bins`   | `hist_find_ndim`    | S1 | EXACT | up-to-3-D histogram cube |
//! | `hist_cum IN OUT`                | `hist_cum`          | S1 | EXACT | running sum along the histogram |
//! | `hist_norm IN OUT`               | `hist_norm`         | S1 | BOUNDED-TOL | band maxima scaled to the max index (cumulative-norm rounding ≤1 LSB vs vips) |
//! | `hist_match IN REF OUT`          | `hist_match`        | S2 | GOLDEN-ONLY | core emits a uchar index LUT, vips a uint LUT — mappings diverge wholesale (measured max-abs-diff 254); no vips cross-oracle |
//! | `hist_plot IN OUT`               | `hist_plot`         | S1 | GOLDEN-ONLY | core plots `max+1` rows, vips `max` — heights never match; no vips cross-oracle |
//! | `hist_entropy IN`                | `hist_entropy`      | S3 | BOUNDED-TOL | Shannon entropy (bits), vips numeric print |
//! | `hist_ismonotonic IN`            | `hist_ismonotonic`  | S3 | EXACT | boolean (`TRUE`/`FALSE`, vips print form) |
//! | `hist_equal IN OUT`             | `hist_equal`        | S1 | BOUNDED-TOL | global equalisation (vips `--band` not in core) |
//! | `hist_local IN OUT W H --max-slope` | `hist_local`    | S1 | GOLDEN-ONLY | window/border algo matches vips only at 3×3 (5×5 diff 51, CLAHE diff 60); no vips cross-oracle |
//! | `maplut IN OUT LUT`             | `maplut`            | S2 | EXACT | map samples through a LUT (2nd input) |
//! | `percent IN PERCENT`            | `percent`           | S3 | GOLDEN-ONLY | core = smallest value whose cumulative reaches P%; vips = threshold above which P% lie (core = vips−2); no vips cross-oracle |
//!
//! **Panic-safety (bands B2 lesson).** The core histogram ops read samples with
//! an internal `read_flat` that **panics** on a float raster (it predates the
//! float formats) — even the `try_*` forms do, since the panic is below the
//! typed-error layer. Every handler therefore rejects a non-integer input with a
//! typed exit-1 error via [`require_integer_raster`] *before* touching the core,
//! and every numeric range conversion (e.g. the `--band` `i64 → u32`) yields a
//! typed error rather than an `unwrap`/`as`-cast abort (`CLI_CONTRACT.md` §8).
//!
//! The three fixed-order two-input commands (`hist_find_indexed`, `hist_match`,
//! `maplut`) declare their inputs as explicit positionals in vips's exact order
//! rather than the variadic [`io::inputs_and_out`] idiom — vips fixes their
//! arity at two images, and `maplut`'s output sits BETWEEN its two inputs
//! (`maplut in out lut`).
//
// @doc-command:begin name=hist_find about="Find the per-band value histogram of an image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=hist_find
// @doc-command:begin name=hist_find_indexed about="Sum image samples into bins selected by an index image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=hist_find_indexed
// @doc-command:begin name=hist_find_ndim about="Find an up-to-3-dimensional histogram of an image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=hist_find_ndim
// @doc-command:begin name=hist_cum about="Form the cumulative histogram (running sum)." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=hist_cum
// @doc-command:begin name=hist_norm about="Normalise a histogram so each band's maximum equals the max index." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=hist_norm
// @doc-command:begin name=hist_match about="Build a LUT matching one histogram to a reference histogram." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=hist_match
// @doc-command:begin name=hist_plot about="Plot a one-band histogram as a bar-graph image." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=hist_plot
// @doc-command:begin name=hist_entropy about="Print the Shannon entropy (bits) of a histogram." \
//     slot-order=load,apply imports-base=decode_file
// @doc-command:end name=hist_entropy
// @doc-command:begin name=hist_ismonotonic about="Print whether a histogram is monotonically non-decreasing." \
//     slot-order=load,apply imports-base=decode_file
// @doc-command:end name=hist_ismonotonic
// @doc-command:begin name=hist_equal about="Histogram-equalise an image (each band independently)." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=hist_equal
// @doc-command:begin name=hist_local about="Local (CLAHE) histogram equalisation over a sliding window." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=hist_local
// @doc-command:begin name=maplut about="Map every sample of an image through a look-up table." \
//     slot-order=load,apply,save imports-base=decode_file,save_file
// @doc-command:end name=maplut
// @doc-command:begin name=percent about="Print the threshold below which a given percent of pixels lie." \
//     slot-order=load,apply imports-base=decode_file
// @doc-command:end name=percent

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use clap::{Arg, ArgMatches, Command, value_parser};
use libviprs::Raster;

use super::{CommandMeta, OracleClass, Shape, io};

/// Static command metadata for the family (name → shape + oracle class), used
/// by `__dump-commands` and by the dispatcher (`CLI_CONTRACT.md` §6).
pub fn metas() -> Vec<CommandMeta> {
    vec![
        CommandMeta {
            name: "hist_find",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "hist_find_indexed",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "hist_find_ndim",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            name: "hist_cum",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            // BOUNDED-TOL (≤1 LSB): normalising a cumulative histogram rounds
            // `(v * (n-1) / max)` ±1 apart from vips on some entries.
            name: "hist_norm",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            // GOLDEN-ONLY: the core emits a `uchar` index LUT where vips emits a
            // `uint` LUT; the mappings diverge wholesale (measured max-abs-diff
            // 254 — see cli_histogram_diff.rs). No vips cross-oracle exists; the
            // differential pins the core against a viprs-generated reference.
            name: "hist_match",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::GoldenOnly,
        },
        CommandMeta {
            // GOLDEN-ONLY: the core plots `max+1` graph rows where vips plots
            // `max`, so the raster dimensions never match. No vips cross-oracle.
            name: "hist_plot",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::GoldenOnly,
        },
        CommandMeta {
            // BOUNDED-TOL: a floating log2 scalar, compared with a relative eps.
            name: "hist_entropy",
            shape: Shape::StdoutScalar,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            name: "hist_ismonotonic",
            shape: Shape::StdoutScalar,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            // BOUNDED-TOL (≤1 LSB): equalisation LUT rounding vs vips.
            name: "hist_equal",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::BoundedTol,
        },
        CommandMeta {
            // GOLDEN-ONLY: the core's sliding-window / border algorithm matches
            // vips only for a 3×3 window; larger windows diverge (measured
            // max-abs-diff 51 at 5×5) and the CLAHE `--max-slope` path diverges
            // further (60 — see cli_histogram_diff.rs). No vips cross-oracle.
            name: "hist_local",
            shape: Shape::ImageToImage,
            oracle_class: OracleClass::GoldenOnly,
        },
        CommandMeta {
            name: "maplut",
            shape: Shape::NImageToImage,
            oracle_class: OracleClass::Exact,
        },
        CommandMeta {
            // GOLDEN-ONLY: the core returns the smallest value whose cumulative
            // count reaches P% (at-or-below); vips returns the threshold above
            // which P% of pixels lie. The definitions differ (measured core =
            // vips − 2 on a dense ramp). No vips cross-oracle.
            name: "percent",
            shape: Shape::StdoutScalar,
            oracle_class: OracleClass::GoldenOnly,
        },
    ]
}

/// The clap commands this family contributes to the assembled CLI.
///
/// Every command loads at least one image, so each carries the shared
/// decode-limit flags via [`io::with_decode_limit_args`].
pub fn commands() -> Vec<Command> {
    vec![
        io::with_decode_limit_args(
            Command::new("hist_find")
                .about("Find the per-band value histogram of an image.")
                .arg(Arg::new("IN").required(true).help("Input image (unsigned 8/16-bit)"))
                .arg(Arg::new("OUT").required(true).help("Output histogram"))
                .arg(
                    Arg::new("band")
                        .long("band")
                        .value_name("N")
                        .default_value("-1")
                        // vips's band range is -1..=100000 (min -1 = all bands,
                        // max 100000). Reject anything outside it at parse time
                        // for strict parity — vips rejects both a sub-`-1` band
                        // and a band above 100000 at parse (B5 strict parity).
                        .value_parser(value_parser!(i64).range(-1..=100000))
                        .help("Find the histogram of this band only (-1..=100000; -1 = all bands, the default)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("hist_find_indexed")
                .about(
                    "Sum image samples into bins selected by an index image. \
                     Bins combine by SUM only (vips's max/min combine modes are \
                     not supported by the core).",
                )
                .arg(Arg::new("IN").required(true).help("Input image (unsigned 8/16-bit)"))
                .arg(
                    Arg::new("INDEX")
                        .required(true)
                        .help("One-band index image selecting the output bin per pixel"),
                )
                .arg(Arg::new("OUT").required(true).help("Output histogram")),
        ),
        io::with_decode_limit_args(
            Command::new("hist_find_ndim")
                .about("Find an up-to-3-dimensional histogram of an image.")
                .arg(Arg::new("IN").required(true).help("Input image (unsigned 8/16-bit, 1-3 bands)"))
                .arg(Arg::new("OUT").required(true).help("Output histogram cube"))
                .arg(
                    Arg::new("bins")
                        .long("bins")
                        .value_name("N")
                        .default_value("10")
                        // vips's declared range is 1..=65536.
                        .value_parser(value_parser!(u32).range(1..=65536))
                        .help("Number of bins in each dimension (1..=65536; default 10)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("hist_cum")
                .about("Form the cumulative histogram (running sum).")
                .arg(Arg::new("IN").required(true).help("Input histogram"))
                .arg(Arg::new("OUT").required(true).help("Output cumulative histogram")),
        ),
        io::with_decode_limit_args(
            Command::new("hist_norm")
                .about("Normalise a histogram so each band's maximum equals the max index.")
                .arg(Arg::new("IN").required(true).help("Input histogram"))
                .arg(Arg::new("OUT").required(true).help("Output normalised histogram")),
        ),
        io::with_decode_limit_args(
            Command::new("hist_match")
                .about("Build a LUT matching one histogram to a reference histogram.")
                .arg(Arg::new("IN").required(true).help("Input histogram"))
                .arg(Arg::new("REF").required(true).help("Reference histogram"))
                .arg(Arg::new("OUT").required(true).help("Output matching LUT")),
        ),
        io::with_decode_limit_args(
            Command::new("hist_plot")
                .about("Plot a one-band histogram as a bar-graph image.")
                .arg(Arg::new("IN").required(true).help("Input one-band histogram"))
                .arg(Arg::new("OUT").required(true).help("Output plot image")),
        ),
        io::with_decode_limit_args(
            Command::new("hist_entropy")
                .about("Print the Shannon entropy (bits) of a histogram.")
                .arg(Arg::new("IN").required(true).help("Input histogram image")),
        ),
        io::with_decode_limit_args(
            Command::new("hist_ismonotonic")
                .about("Print whether a histogram is monotonically non-decreasing.")
                .arg(Arg::new("IN").required(true).help("Input histogram image")),
        ),
        io::with_decode_limit_args(
            Command::new("hist_equal")
                .about("Histogram-equalise an image (each band independently).")
                .arg(Arg::new("IN").required(true).help("Input image (unsigned 8/16-bit)"))
                .arg(Arg::new("OUT").required(true).help("Output equalised image")),
        ),
        io::with_decode_limit_args(
            Command::new("hist_local")
                .about("Local (CLAHE) histogram equalisation over a sliding window.")
                .arg(Arg::new("IN").required(true).help("Input image (unsigned 8/16-bit)"))
                .arg(Arg::new("OUT").required(true).help("Output image"))
                .arg(
                    Arg::new("WIDTH")
                        .required(true)
                        // vips's window range is 1..=100000000 (a `gint`). Cap
                        // both bounds for strict parity AND to remove an overflow
                        // / effectively-infinite-loop vector: an unbounded window
                        // (e.g. u32::MAX) makes the core's `ww * wh` window area
                        // overflow i64 and its per-pixel window loop never return.
                        .value_parser(value_parser!(u32).range(1..=100_000_000))
                        .help("Window width in pixels (1..=100000000)"),
                )
                .arg(
                    Arg::new("HEIGHT")
                        .required(true)
                        .value_parser(value_parser!(u32).range(1..=100_000_000))
                        .help("Window height in pixels (1..=100000000)"),
                )
                .arg(
                    Arg::new("max-slope")
                        .long("max-slope")
                        .value_name("N")
                        .default_value("0")
                        // vips's max-slope is a gint in 0..=100 (0 = unlimited).
                        .value_parser(value_parser!(i64).range(0..=100))
                        .help("CLAHE contrast limit (0..=100; 0 = unlimited, the default)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("maplut")
                .about(
                    "Map every sample of an image through a look-up table. \
                     The LUT applies to ALL bands (vips's per-band `--band` \
                     selection is not supported by the core).",
                )
                .arg(Arg::new("IN").required(true).help("Input image (unsigned 8/16-bit)"))
                .arg(Arg::new("OUT").required(true).help("Output mapped image"))
                .arg(
                    Arg::new("LUT")
                        .required(true)
                        .help("Look-up table image (histogram-shaped: Nx1 or 1xN)"),
                ),
        ),
        io::with_decode_limit_args(
            Command::new("percent")
                .about("Print the threshold below which a given percent of pixels lie.")
                .arg(Arg::new("IN").required(true).help("Input image (unsigned 8/16-bit)"))
                .arg(
                    Arg::new("PERCENT")
                        .required(true)
                        // vips's percent is a gdouble in 0..=100; the core
                        // rejects an out-of-range value with a typed exit-1
                        // error (clap has no f64 range parser).
                        .value_parser(value_parser!(f64))
                        .help("Percent of pixels (0..=100)"),
                ),
        ),
    ]
}

/// Dispatch a matched histogram subcommand to its handler.
pub fn run(name: &str, m: &ArgMatches) -> Result<()> {
    match name {
        "hist_find" => run_hist_find(m),
        "hist_find_indexed" => run_hist_find_indexed(m),
        "hist_find_ndim" => run_hist_find_ndim(m),
        "hist_cum" => run_hist_cum(m),
        "hist_norm" => run_hist_norm(m),
        "hist_match" => run_hist_match(m),
        "hist_plot" => run_hist_plot(m),
        "hist_entropy" => run_hist_entropy(m),
        "hist_ismonotonic" => run_hist_ismonotonic(m),
        "hist_equal" => run_hist_equal(m),
        "hist_local" => run_hist_local(m),
        "maplut" => run_maplut(m),
        "percent" => run_percent(m),
        other => bail!("histogram family has no command {other:?}"),
    }
}

/// Read a required positional string argument.
fn pos<'a>(m: &'a ArgMatches, id: &str) -> &'a str {
    m.get_one::<String>(id)
        .map(String::as_str)
        .expect("clap guarantees a required positional is present")
}

/// Reject a raster the core histogram ops cannot read.
///
/// The core `read_flat` helper (below the `try_*` typed-error layer) **panics**
/// on any sample depth other than 1 or 2 bytes — a float raster would abort the
/// process (exit 101). Guard here so a float / non-integer input is a typed
/// exit-1 error instead (bands B2 / `CLI_CONTRACT.md` §8).
fn require_integer_raster(r: &Raster, what: &str) -> Result<()> {
    let bpc = r.format().bytes_per_channel();
    if bpc != 1 && bpc != 2 {
        bail!(
            "{what} must be an unsigned 8- or 16-bit integer image \
             (the histogram operations do not support float / {:?} rasters); \
             cast it with `viprs cast` first",
            r.format()
        );
    }
    Ok(())
}

/// `hist_find IN OUT --band N` — S1; per-band value histogram (or one band).
fn run_hist_find(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    // -1 (the default) = all bands. A non-negative band selects one band; the
    // i64 → u32 conversion is a typed exit-1 error, never a `try_from` abort.
    let band_raw = *m.get_one::<i64>("band").expect("clap default -1");
    let band: Option<u32> = if band_raw < 0 {
        None
    } else {
        Some(
            u32::try_from(band_raw)
                .map_err(|_| anyhow!("--band {band_raw} out of range (max {})", u32::MAX))?,
        )
    };

    // @doc-snippet:begin command=hist_find slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=hist_find slot=load
    require_integer_raster(&raster, "the hist_find input")?;

    // @doc-snippet:begin command=hist_find slot=apply
    // @doc-test: histogram.rs::hist_find_counts_values:1153 repo=libviprs
    let out = match band {
        None => raster.try_hist_find()?,
        Some(b) => raster.try_hist_find_band(b)?,
    };
    // @doc-snippet:end command=hist_find slot=apply

    // @doc-snippet:begin command=hist_find slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=hist_find slot=save
    Ok(())
}

/// `hist_find_indexed IN INDEX OUT` — S2 (2 fixed inputs); per-index sample sums.
fn run_hist_find_indexed(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let index_path = PathBuf::from(pos(m, "INDEX"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=hist_find_indexed slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    let index = io::load(&index_path, &limits)?;
    // @doc-snippet:end command=hist_find_indexed slot=load
    require_integer_raster(&raster, "the hist_find_indexed input")?;
    require_integer_raster(&index, "the hist_find_indexed index")?;

    // @doc-snippet:begin command=hist_find_indexed slot=apply
    // @doc-test: histogram.rs::hist_find_indexed_sums:1226 repo=libviprs
    let out = raster.try_hist_find_indexed(&index)?;
    // @doc-snippet:end command=hist_find_indexed slot=apply

    // @doc-snippet:begin command=hist_find_indexed slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=hist_find_indexed slot=save
    Ok(())
}

/// `hist_find_ndim IN OUT --bins N` — S1; up-to-3-D histogram cube.
fn run_hist_find_ndim(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let bins = *m.get_one::<u32>("bins").expect("clap default 10");

    // @doc-snippet:begin command=hist_find_ndim slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=hist_find_ndim slot=load
    require_integer_raster(&raster, "the hist_find_ndim input")?;

    // @doc-snippet:begin command=hist_find_ndim slot=apply
    // @doc-test: histogram.rs::hist_find_ndim_default_and_single_bin:1274 repo=libviprs
    let out = raster.try_hist_find_ndim(Some(bins))?;
    // @doc-snippet:end command=hist_find_ndim slot=apply

    // @doc-snippet:begin command=hist_find_ndim slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=hist_find_ndim slot=save
    Ok(())
}

/// `hist_cum IN OUT` — S1; cumulative (running-sum) histogram.
fn run_hist_cum(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=hist_cum slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=hist_cum slot=load
    require_integer_raster(&raster, "the hist_cum input")?;

    // @doc-snippet:begin command=hist_cum slot=apply
    // @doc-test: histogram.rs::hist_cum_running_sum:1360 repo=libviprs
    let out = raster.try_hist_cum()?;
    // @doc-snippet:end command=hist_cum slot=apply

    // @doc-snippet:begin command=hist_cum slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=hist_cum slot=save
    Ok(())
}

/// `hist_norm IN OUT` — S1; scale each band's maximum to the max index.
fn run_hist_norm(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=hist_norm slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=hist_norm slot=load
    require_integer_raster(&raster, "the hist_norm input")?;

    // @doc-snippet:begin command=hist_norm slot=apply
    // @doc-test: histogram.rs::hist_norm_scales_to_max_index:1400 repo=libviprs
    let out = raster.try_hist_norm()?;
    // @doc-snippet:end command=hist_norm slot=apply

    // @doc-snippet:begin command=hist_norm slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=hist_norm slot=save
    Ok(())
}

/// `hist_match IN REF OUT` — S2 (2 fixed inputs); CDF-match LUT.
fn run_hist_match(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let ref_path = PathBuf::from(pos(m, "REF"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=hist_match slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    let reference = io::load(&ref_path, &limits)?;
    // @doc-snippet:end command=hist_match slot=load
    require_integer_raster(&raster, "the hist_match input")?;
    require_integer_raster(&reference, "the hist_match reference")?;

    // @doc-snippet:begin command=hist_match slot=apply
    // @doc-test: histogram.rs::hist_match_identity:1470 repo=libviprs
    let out = raster.try_hist_match(&reference)?;
    // @doc-snippet:end command=hist_match slot=apply

    // @doc-snippet:begin command=hist_match slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=hist_match slot=save
    Ok(())
}

/// `hist_plot IN OUT` — S1; bar-graph raster of a one-band histogram.
fn run_hist_plot(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=hist_plot slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=hist_plot slot=load
    require_integer_raster(&raster, "the hist_plot input")?;

    // @doc-snippet:begin command=hist_plot slot=apply
    // @doc-test: histogram.rs::hist_plot_bar_graph:1540 repo=libviprs
    let out = raster.try_hist_plot()?;
    // @doc-snippet:end command=hist_plot slot=apply

    // @doc-snippet:begin command=hist_plot slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=hist_plot slot=save
    Ok(())
}

/// `hist_entropy IN` — S3 image→stdout-scalar; Shannon entropy in bits.
fn run_hist_entropy(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));

    // @doc-snippet:begin command=hist_entropy slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=hist_entropy slot=load
    require_integer_raster(&raster, "the hist_entropy input")?;

    // @doc-snippet:begin command=hist_entropy slot=apply
    // @doc-test: histogram.rs::hist_entropy_bits:1600 repo=libviprs
    let entropy = raster.try_hist_entropy()?;
    // @doc-snippet:end command=hist_entropy slot=apply

    // vips prints entropy in its numeric format (`4.000000`); the harness
    // float-parses with an epsilon rather than comparing text.
    println!("{}", fmt_vips_double(entropy));
    Ok(())
}

/// `hist_ismonotonic IN` — S3 image→stdout-scalar; boolean monotonicity.
fn run_hist_ismonotonic(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));

    // @doc-snippet:begin command=hist_ismonotonic slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=hist_ismonotonic slot=load
    require_integer_raster(&raster, "the hist_ismonotonic input")?;

    // @doc-snippet:begin command=hist_ismonotonic slot=apply
    // @doc-test: histogram.rs::hist_ismonotonic_bool:1660 repo=libviprs
    let monotonic = raster.try_hist_ismonotonic()?;
    // @doc-snippet:end command=hist_ismonotonic slot=apply

    // Print vips's exact boolean form (`TRUE` / `FALSE`) so the differential can
    // compare the printed bool directly (OP_MAP.md: "harness parses vips's
    // printed bool form").
    println!("{}", if monotonic { "TRUE" } else { "FALSE" });
    Ok(())
}

/// `hist_equal IN OUT` — S1; global histogram equalisation (all bands).
fn run_hist_equal(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));

    // @doc-snippet:begin command=hist_equal slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=hist_equal slot=load
    // `hist_equal` is infallible in the core but its inner read_flat panics on a
    // float raster, so the integer guard is mandatory here too.
    require_integer_raster(&raster, "the hist_equal input")?;

    // @doc-snippet:begin command=hist_equal slot=apply
    // @doc-test: histogram.rs::hist_equal_raises_contrast:1720 repo=libviprs
    let out = raster.hist_equal();
    // @doc-snippet:end command=hist_equal slot=apply

    // @doc-snippet:begin command=hist_equal slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=hist_equal slot=save
    Ok(())
}

/// `hist_local IN OUT WIDTH HEIGHT --max-slope N` — S1; CLAHE equalisation.
fn run_hist_local(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let width = *m.get_one::<u32>("WIDTH").expect("required");
    let height = *m.get_one::<u32>("HEIGHT").expect("required");
    // vips default 0 = unlimited contrast (= core `None`). The `0..=100` range
    // makes the `i64 → f64` widening always in-range and abort-free.
    let max_slope = match *m.get_one::<i64>("max-slope").expect("clap default 0") {
        0 => None,
        v => Some(v as f64),
    };

    // @doc-snippet:begin command=hist_local slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=hist_local slot=load
    require_integer_raster(&raster, "the hist_local input")?;

    // @doc-snippet:begin command=hist_local slot=apply
    // @doc-test: histogram.rs::hist_local_window_equalises:1780 repo=libviprs
    let out = raster.try_hist_local(width, height, max_slope)?;
    // @doc-snippet:end command=hist_local slot=apply

    // @doc-snippet:begin command=hist_local slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=hist_local slot=save
    Ok(())
}

/// `maplut IN OUT LUT` — S2 (LUT is the 2nd input, after OUT in vips order).
fn run_maplut(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let out_path = PathBuf::from(pos(m, "OUT"));
    let lut_path = PathBuf::from(pos(m, "LUT"));

    // @doc-snippet:begin command=maplut slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    let lut = io::load(&lut_path, &limits)?;
    // @doc-snippet:end command=maplut slot=load
    require_integer_raster(&raster, "the maplut input")?;
    require_integer_raster(&lut, "the maplut LUT")?;

    // @doc-snippet:begin command=maplut slot=apply
    // @doc-test: histogram.rs::maplut_maps_through_table:1850 repo=libviprs
    let out = raster.try_maplut(&lut)?;
    // @doc-snippet:end command=maplut slot=apply

    // @doc-snippet:begin command=maplut slot=save imports=save_file
    io::save(&out, &out_path)?;
    // @doc-snippet:end command=maplut slot=save
    Ok(())
}

/// `percent IN PERCENT` — S3 image→stdout-scalar; percentile threshold (int).
fn run_percent(m: &ArgMatches) -> Result<()> {
    let limits = io::decode_limits(m);
    let in_path = PathBuf::from(pos(m, "IN"));
    let percent = *m.get_one::<f64>("PERCENT").expect("required");

    // @doc-snippet:begin command=percent slot=load imports=decode_file
    let raster = io::load(&in_path, &limits)?;
    // @doc-snippet:end command=percent slot=load
    require_integer_raster(&raster, "the percent input")?;

    // @doc-snippet:begin command=percent slot=apply
    // @doc-test: histogram.rs::percent_threshold:1920 repo=libviprs
    let threshold = raster.try_percent(percent)?;
    // @doc-snippet:end command=percent slot=apply

    // The threshold is an integer sample value; `f64` Display prints it without
    // a fractional part (`136`), matching vips's `gint` print. The harness
    // float-parses either form.
    println!("{threshold}");
    Ok(())
}

/// Format a scalar in the vips numeric print format (`hist_entropy` prints
/// `4.000000`, mirroring `avg`; `CLI_CONTRACT.md` §3). The differential harness
/// float-parses the value with an epsilon rather than comparing text.
fn fmt_vips_double(v: f64) -> String {
    format!("{v:.6}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(name: &str) -> Command {
        commands()
            .into_iter()
            .find(|c| c.get_name() == name)
            .unwrap_or_else(|| panic!("no command {name}"))
    }

    #[test]
    fn commands_and_metas_agree() {
        let cmd_names: Vec<String> = commands()
            .iter()
            .map(|c| c.get_name().to_string())
            .collect();
        let meta_names: Vec<&str> = metas().iter().map(|m| m.name).collect();
        for name in &meta_names {
            assert!(
                cmd_names.iter().any(|c| c == name),
                "meta {name} has no command"
            );
        }
        assert_eq!(cmd_names.len(), meta_names.len());
        assert_eq!(
            meta_names.len(),
            13,
            "the histogram family has thirteen commands"
        );
    }

    #[test]
    fn hist_find_parses_in_out_and_band_default() {
        let m = cmd("hist_find")
            .try_get_matches_from(["hist_find", "in.png", "out.v"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "OUT"), "out.v");
        // Default -1 = all bands.
        assert_eq!(*m.get_one::<i64>("band").unwrap(), -1);
    }

    #[test]
    fn hist_find_band_below_minus_one_is_rejected() {
        // vips's minimum band is -1; the `-1..` value_parser rejects anything
        // lower at parse time (strict parity — no sub-`-1` extension).
        assert!(
            cmd("hist_find")
                .try_get_matches_from(["hist_find", "in.png", "out.v", "--band", "-2"])
                .is_err(),
            "a --band below -1 must be rejected"
        );
    }

    #[test]
    fn hist_find_band_above_max_is_rejected() {
        // vips's band maximum is 100000; the `-1..=100000` value_parser rejects
        // anything above it at parse time (strict parity — no core-only extension
        // of the upper bound leaks into the CLI surface). 100001 exceeds any real
        // band count, so no legitimate case is lost.
        assert!(
            cmd("hist_find")
                .try_get_matches_from(["hist_find", "in.png", "out.v", "--band", "100001"])
                .is_err(),
            "a --band above 100000 must be rejected"
        );
    }

    #[test]
    fn hist_find_indexed_parses_three_positionals_in_vips_order() {
        let m = cmd("hist_find_indexed")
            .try_get_matches_from(["hist_find_indexed", "in.png", "idx.png", "out.v"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "INDEX"), "idx.png");
        assert_eq!(pos(&m, "OUT"), "out.v");
    }

    #[test]
    fn hist_find_ndim_bins_defaults_and_bounds() {
        let m = cmd("hist_find_ndim")
            .try_get_matches_from(["hist_find_ndim", "in.png", "out.v"])
            .unwrap();
        assert_eq!(*m.get_one::<u32>("bins").unwrap(), 10);
        // vips's range is 1..=65536: 0 and 65537 are both rejected at parse time.
        assert!(
            cmd("hist_find_ndim")
                .try_get_matches_from(["hist_find_ndim", "in.png", "out.v", "--bins", "0"])
                .is_err(),
            "--bins 0 must be rejected"
        );
        assert!(
            cmd("hist_find_ndim")
                .try_get_matches_from(["hist_find_ndim", "in.png", "out.v", "--bins", "65537"])
                .is_err(),
            "--bins above 65536 must be rejected"
        );
    }

    #[test]
    fn maplut_positional_order_is_in_out_lut() {
        // vips's own order is `maplut in out lut` — the output sits BETWEEN the
        // two inputs.
        let m = cmd("maplut")
            .try_get_matches_from(["maplut", "in.png", "out.png", "lut.v"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(pos(&m, "OUT"), "out.png");
        assert_eq!(pos(&m, "LUT"), "lut.v");
    }

    #[test]
    fn hist_local_window_and_max_slope() {
        let m = cmd("hist_local")
            .try_get_matches_from(["hist_local", "in.png", "out.png", "3", "5"])
            .unwrap();
        assert_eq!(*m.get_one::<u32>("WIDTH").unwrap(), 3);
        assert_eq!(*m.get_one::<u32>("HEIGHT").unwrap(), 5);
        // Default max-slope 0 = unlimited.
        assert_eq!(*m.get_one::<i64>("max-slope").unwrap(), 0);
        // vips's window minimum is 1; 0 is rejected.
        assert!(
            cmd("hist_local")
                .try_get_matches_from(["hist_local", "in.png", "out.png", "0", "5"])
                .is_err(),
            "a zero window width must be rejected"
        );
        // vips's window maximum is 100000000; anything above it is rejected at
        // parse time — this also removes the i64 window-area overflow / infinite
        // window-loop vector (a single u32::MAX arg would otherwise hang).
        assert!(
            cmd("hist_local")
                .try_get_matches_from(["hist_local", "in.png", "out.png", "200000000", "5"])
                .is_err(),
            "a window width above 100000000 must be rejected"
        );
        // vips's max-slope range is 0..=100.
        assert!(
            cmd("hist_local")
                .try_get_matches_from([
                    "hist_local",
                    "in.png",
                    "out.png",
                    "3",
                    "3",
                    "--max-slope",
                    "101",
                ])
                .is_err(),
            "--max-slope above 100 must be rejected"
        );
    }

    #[test]
    fn percent_parses_the_percentile() {
        let m = cmd("percent")
            .try_get_matches_from(["percent", "in.png", "25"])
            .unwrap();
        assert_eq!(pos(&m, "IN"), "in.png");
        assert_eq!(*m.get_one::<f64>("PERCENT").unwrap(), 25.0);
    }

    #[test]
    fn scalar_only_commands_take_no_out() {
        // hist_entropy / hist_ismonotonic / percent are S3: one input, no OUT.
        for name in ["hist_entropy", "hist_ismonotonic"] {
            let m = cmd(name).try_get_matches_from([name, "in.v"]).unwrap();
            assert_eq!(pos(&m, "IN"), "in.v");
        }
    }

    #[test]
    fn fmt_vips_double_matches_contract_example() {
        assert_eq!(fmt_vips_double(4.0), "4.000000");
        assert_eq!(fmt_vips_double(0.5), "0.500000");
    }

    #[test]
    fn require_integer_raster_rejects_float() {
        use libviprs::PixelFormat;
        let fmt = PixelFormat::with_channels(1, 4).unwrap();
        let r = Raster::from_f32_samples(2, 1, fmt, &[0.0, 1.0]).unwrap();
        let err = require_integer_raster(&r, "the test input").unwrap_err();
        assert!(err.to_string().contains("integer image"), "got: {err}");
        // An 8-bit raster is accepted.
        let g = Raster::zeroed(2, 2, PixelFormat::Gray8).unwrap();
        assert!(require_integer_raster(&g, "the test input").is_ok());
    }
}
