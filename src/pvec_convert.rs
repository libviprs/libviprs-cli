//! `viprs pvec convert` (libviprs#1566, slice C of libviprs#1557): a DWG to `.pvec` files.
//!
//! ```text
//! viprs pvec convert <drawing.dwg> <out-dir>                 every paper layout, one .pvec each
//! viprs pvec convert <drawing.dwg> <out-dir> --layout A-101  only that layout (repeatable)
//! viprs pvec convert <drawing.dwg> <out-dir> --model         model space, one .pvec
//! viprs pvec convert <drawing.dwg> <out-dir> --regions       also rooms and corridors
//! viprs pvec convert <drawing.dwg> <out-dir> --geometry-only no layer, line style or provenance
//! viprs pvec convert <drawing.dwg> <out-dir> --annotations   also text, hatches and solid fills
//! viprs pvec convert <drawing.dwg> <out-dir> --regions --element-rule layer:A-WALL*=wall
//! viprs pvec convert <drawing.dwg> <out-dir> --thumbnail 512  also a PNG of each sheet
//! viprs pvec convert <drawing.dwg> <out-dir> --profile strict --scale 0.05   a profile, and a scale
//! viprs pvec convert <page.pdf> <out-dir> --page 2             one PDF page, as <stem>.pvec
//! viprs pvec layouts <drawing.dwg>                           the paper layouts, nothing converted
//! ```
//!
//! A thin call onto `libviprs::pvec_convert`: `PvecLayoutSet` for the sheets, `PvecConversion` for
//! model space, and `paper_sheets` for `viprs pvec layouts` (libviprs-cli#150). A sheet is `<stem>.<NN>-<slug>.pvec` with its report beside it and the set index
//! `<stem>.sheets.json` written last; model space is `<stem>.pvec` and its report. A PDF page goes through
//! `PdfConversion` (`pvec_convert/pdf.rs`) into `<stem>.pvec` and its report too (libviprs-cli#151). Which
//! input it is comes from the name: `.pdf`, any case, is a PDF and anything else a DWG, and a flag that belongs to
//! the other kind (`pvec_convert/knobs.rs` has them, grouped) is a usage error. This file parses arguments, prints
//! one line per sheet and picks the exit code.
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

mod knobs;
#[cfg(all(feature = "pvec-convert", feature = "pvec-raster"))]
mod pdf;

use knobs::{
    CalibrationArgs, CapabilityArgs, PdfArgs, RegionArgs, SourceLimitArgs, ViewArgs, WriterArgs,
};

/// The widest picture `--thumbnail` and `viprs pvec thumbnail` draw, in pixels.
pub(crate) const MAX_THUMBNAIL_WIDTH: u32 = 4096;

/// `viprs pvec layouts`.
#[derive(Args)]
pub(crate) struct LayoutsArgs {
    /// The drawing (a DWG).
    drawing: PathBuf,

    /// Print `{"v": 1, "layouts": [...]}` instead of one line per layout. A layout is an
    /// object with `tab_order` (1-based, the number `pvec convert` gives its file), `layout`,
    /// `sheet` (whether it would be converted) and `skip`: `null` for a sheet, else `"empty"`
    /// or `"empty-after-view"`.
    #[arg(long)]
    json: bool,
}

/// One `--element-rule`: what it matches and whether the lines it matches are walls or columns.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "pvec-convert"), allow(dead_code))]
pub(crate) struct ElementRule {
    matcher: Matcher,
    wall: bool,
}

/// What an `--element-rule` matches against, as the core's rule list takes it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(not(feature = "pvec-convert"), allow(dead_code))]
enum Matcher {
    Layer(String),
    Block(String),
    Colour(u8),
    Lineweight(u16),
}

/// `KIND:MATCH=CLASS`, for `--element-rule`. KIND is `layer` or `block` (a glob with `*` and
/// `?`, case ignored), `colour` (an AutoCAD colour index, 1 to 255) or `lineweight` (hundredths of
/// a millimetre); CLASS is `wall` or `column`.
fn parse_element_rule(spec: &str) -> Result<ElementRule, String> {
    let shape = "expected KIND:MATCH=CLASS, such as layer:A-WALL*=wall";
    let (left, class) = spec.rsplit_once('=').ok_or(shape)?;
    let (kind, pattern) = left.split_once(':').ok_or(shape)?;
    let wall = match class.to_ascii_lowercase().as_str() {
        "wall" => true,
        "column" => false,
        _ => return Err(format!("the class is `wall` or `column`, not {class:?}")),
    };
    if pattern.is_empty() {
        return Err(format!("the match after `{kind}:` is empty"));
    }
    let matcher = match kind.to_ascii_lowercase().as_str() {
        "layer" => Matcher::Layer(pattern.to_owned()),
        "block" => Matcher::Block(pattern.to_owned()),
        "colour" | "color" => match pattern.parse::<u8>() {
            Ok(n) if n >= 1 => Matcher::Colour(n),
            _ => {
                return Err(format!(
                    "a colour is an AutoCAD colour index from 1 to 255, not {pattern:?}"
                ));
            }
        },
        "lineweight" => pattern
            .parse::<u16>()
            .map(Matcher::Lineweight)
            .map_err(|_| {
                format!(
                    "a lineweight is a whole number of hundredths of a millimetre, not {pattern:?}"
                )
            })?,
        _ => {
            return Err(format!(
                "the kind is `layer`, `block`, `colour` or `lineweight`, not {kind:?}"
            ));
        }
    };
    Ok(ElementRule { matcher, wall })
}

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

    /// Also carry text (TEXT, MTEXT, ATTDEF, a MULTILEADER's text, a TOLERANCE's frame text) in the
    /// text stream and HATCH and SOLID in the hatch stream. Off by default, and it needs the line
    /// style stream, so it cannot be combined with --geometry-only.
    #[arg(long, conflicts_with = "geometry_only")]
    annotations: bool,

    /// Which lines are walls and columns for --regions (repeatable): KIND:MATCH=CLASS. KIND is
    /// `layer` or `block` (a glob with * and ?, case ignored), `colour` (an AutoCAD colour
    /// index, 1 to 255) or `lineweight` (hundredths of a millimetre); CLASS is `wall` or
    /// `column`. Block rules win over layer rules, then colour, then lineweight, and the earlier
    /// rule of a kind wins. The rules replace the default profile (A-WALL*, S-WALL*, A-COL*,
    /// S-COL*, COLUMN*), so name every one you want. Needs --regions.
    #[arg(
        long = "element-rule",
        value_name = "KIND:MATCH=CLASS",
        requires = "regions",
        value_parser = parse_element_rule
    )]
    element_rules: Vec<ElementRule>,

    /// Also draw each converted sheet as a PNG WIDTH pixels wide, written beside its `.pvec` with
    /// the extension `.png`. It draws strokes only, no text, no fills; the height follows the sheet. Needs
    /// the `pvec-raster-thumbnail` feature (in `full`, not in the default build). Not with --model: draw
    /// a model-space file with `viprs pvec thumbnail`, which caps the height.
    #[arg(
        long,
        value_name = "WIDTH",
        value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_THUMBNAIL_WIDTH))
    )]
    thumbnail: Option<u32>,

    #[command(flatten)]
    pdf: PdfArgs,

    #[command(flatten)]
    capability: CapabilityArgs,

    #[command(flatten)]
    calibration: CalibrationArgs,

    #[command(flatten)]
    view: ViewArgs,

    #[command(flatten)]
    limits: SourceLimitArgs,

    #[command(flatten)]
    writer: WriterArgs,

    #[command(flatten)]
    region_tuning: RegionArgs,
}

impl ConvertArgs {
    /// The flags given that only a DWG takes.
    fn dwg_only(&self) -> Vec<&'static str> {
        let own = knobs::given(&[
            (!self.layout.is_empty(), "--layout"),
            (self.model, "--model"),
            (self.unit.is_some(), "--unit"),
            (self.geometry_only, "--geometry-only"),
            (self.regions, "--regions"),
            (self.annotations, "--annotations"),
            (!self.element_rules.is_empty(), "--element-rule"),
            (self.thumbnail.is_some(), "--thumbnail"),
        ]);
        [
            own,
            self.capability.given(),
            self.calibration.given(),
            self.view.given(),
            self.limits.given(),
            self.writer.given(),
            self.region_tuning.given(),
        ]
        .concat()
    }
}

pub(crate) fn run(args: ConvertArgs) {
    if args.model && args.thumbnail.is_some() {
        usage_error(
            "--thumbnail cannot be used with --model",
            "model space has no paper to bound its shape, so one stray far entity would make a huge picture; draw it with `viprs pvec thumbnail FILE OUT.png`, which caps the width and the height",
        );
    }
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
    // A name ending in .pdf, any case, is what sends a conversion down the PDF path.
    let a_pdf = args
        .drawing
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"));
    let misplaced = if a_pdf {
        args.dwg_only()
    } else {
        args.pdf.given()
    };
    if !misplaced.is_empty() {
        let (only, input) = if a_pdf {
            ("DWG", "PDF")
        } else {
            ("PDF", "DWG")
        };
        usage_error(
            &format!(
                "{} only apply to a {only}, and {} is a {input}",
                misplaced.join(", "),
                args.drawing.display()
            ),
            "drop them, or give the other kind of input (a name ending in .pdf is a PDF)",
        );
    }
    if let Some(dir) = &args.writer.spill_dir
        && dir.exists()
        && !dir.is_dir()
    {
        usage_error(
            &format!("--spill-dir {} is not a directory", dir.display()),
            "give a directory; it is created when it does not exist",
        );
    }
    #[cfg(feature = "pvec-convert")]
    {
        if a_pdf {
            #[cfg(feature = "pvec-raster")]
            pdf::run(&args);
            #[cfg(not(feature = "pvec-raster"))]
            crate::operational_error(
                "a PDF page needs the `pvec-raster` feature; rebuild with `--features pvec-raster`",
            );
        } else {
            #[cfg(not(feature = "pvec-raster-thumbnail"))]
            if args.thumbnail.is_some() {
                crate::operational_error(
                    "`--thumbnail` is not in this build; rebuild with `--features pvec-raster-thumbnail`",
                );
            }
            imp::run(&args);
        }
    }
    #[cfg(not(feature = "pvec-convert"))]
    {
        let _ = args;
        crate::operational_error(
            "`viprs pvec convert` is not in this build; rebuild with `--features pvec-convert`",
        );
    }
}

/// `viprs pvec layouts`: the drawing's paper layouts in tab order, and whether each is a sheet,
/// without converting any of them.
pub(crate) fn run_layouts(args: LayoutsArgs) {
    if !args.drawing.is_file() {
        crate::operational_error(&format!(
            "{} does not exist or is not a file",
            args.drawing.display()
        ));
    }
    #[cfg(feature = "pvec-convert")]
    imp::layouts(&args);
    #[cfg(not(feature = "pvec-convert"))]
    {
        let _ = args;
        crate::operational_error(
            "`viprs pvec layouts` is not in this build; rebuild with `--features pvec-convert`",
        );
    }
}

#[cfg(feature = "pvec-convert")]
mod imp {
    use super::*;

    use std::path::Path;

    use libviprs::cad::CadSource;
    use libviprs::pvec::DwgProvider;
    use libviprs::pvec::SourceLimits;
    use libviprs::pvec::SourceUnit;
    use libviprs::pvec::ViewProfile;
    use libviprs::pvec::classify::{ElementClass, ElementRules, RuleMatch};
    use libviprs::pvec::components::StreamSelection;
    use libviprs::pvec::dwg_acadrust::AcadrustProvider;
    use libviprs::pvec::provider::SourceColour;
    use libviprs::pvec::regions::RegionConfig;
    use libviprs::pvec::writer::{CapabilityId, LeafConfig, SpillConfig, WriteError};
    use libviprs::pvec::{InsertLayerRule, LayerOverride, LayerStateRule, ViewportFreezeRule};
    #[allow(deprecated)]
    use libviprs::pvec_convert::GeoreferenceHandoff;
    use libviprs::pvec_convert::{
        Calibration, CapabilityPolicy, ConversionProfile, LayoutSetError, PvecConversion,
        PvecConvertError, PvecLayoutSet, ScaleClaim, ScaleProvenance, SheetSkip, import_sidecar,
        paper_sheets,
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

    /// The regions step's knobs: the lengths are in metres, scaled to the drawing's unit, and the
    /// defaults are the ones the step shipped with.
    pub(super) fn region_config(args: &ConvertArgs, per_metre: f64) -> RegionConfig {
        let r = &args.region_tuning;
        RegionConfig {
            gap_tolerance: r.region_gap.unwrap_or(0.5) * per_metre,
            corridor_max_width: r.region_corridor_width.unwrap_or(2.5) * per_metre,
            corridor_min_aspect: r.region_corridor_aspect.unwrap_or(3.0),
            max_faces: r.region_max_faces.unwrap_or(1000),
            keep_unlabelled: r.region_keep_unlabelled,
        }
    }

    /// The scale the flags state, as a calibration, or `None` when none was given.
    fn calibration(args: &ConvertArgs) -> Result<Option<Calibration>, String> {
        let c = &args.calibration;
        let claim = |metres_per_source_unit, provenance| ScaleClaim {
            metres_per_source_unit,
            provenance,
            uncertainty: c.scale_uncertainty,
            note: c.scale_note.clone().unwrap_or_default(),
        };
        // clap's group makes at most one of these present, and each one's companion with it.
        let scale = match (c.scale, c.plan_scale, c.paper_unit, c.measured, c.distance) {
            (Some(metres), ..) => claim(metres, ScaleProvenance::Manual),
            (None, Some((numerator, denominator)), Some(paper), ..) => claim(
                paper * f64::from(denominator) / f64::from(numerator),
                ScaleProvenance::PlanScale {
                    numerator,
                    denominator,
                    paper_metres_per_unit: paper,
                },
            ),
            (None, None, _, Some([x0, y0, x1, y1]), Some(metres)) => claim(
                metres / (x1 - x0).hypot(y1 - y0),
                ScaleProvenance::Measured {
                    p0: [x0, y0],
                    p1: [x1, y1],
                    metres,
                },
            ),
            _ => return Ok(None),
        };
        Calibration::new(Some(scale), None, None)
            .map(Some)
            .map_err(|e| format!("the scale was refused: {}", with_causes(&e)))
    }

    /// Brings the sidecar's id into the artifact at `artifact`: the calibration the flags state,
    /// with the id as its external id.
    fn import(artifact: &Path, sidecar: &Path, calibration: &Calibration) -> String {
        match import_sidecar(artifact, sidecar, |_| Some(calibration.clone())) {
            Ok(_) => format!("calibration imported from {}", name(sidecar)),
            Err(e) => operational_error(&format!(
                "importing {} into {}: {}",
                sidecar.display(),
                artifact.display(),
                with_causes(&e)
            )),
        }
    }

    /// The reader limits the flags ask for, or `None` when none was given. `SourceLimits` is
    /// non-exhaustive, so it is the library's defaults with each flag written over its field.
    pub(super) fn source_limits(args: &ConvertArgs) -> Option<SourceLimits> {
        let l = &args.limits;
        if l.given().is_empty() {
            return None;
        }
        let mut limits = SourceLimits::default();
        if let Some(v) = l.max_block_depth {
            limits.max_block_depth = v;
        }
        if let Some(v) = l.max_entities {
            limits.max_entities = v;
        }
        if let Some(v) = l.max_polyline_points {
            limits.max_polyline_vertices = usize::try_from(v).unwrap_or(usize::MAX);
        }
        if let Some(v) = l.max_spline_control_points {
            limits.max_spline_control_points = v;
        }
        if let Some(v) = l.max_spline_knots {
            limits.max_spline_knots = v;
        }
        if let Some(v) = l.max_insert_grid {
            limits.max_insert_grid = v;
        }
        if let Some(v) = l.max_expanded_instances {
            limits.max_expanded_instances = v;
        }
        if let Some(v) = l.max_alloc_bytes {
            limits.max_allocation_bytes = v;
        }
        if let Some(v) = l.max_name_bytes {
            limits.max_name_bytes = v;
        }
        Some(limits)
    }

    /// The view profile the flags ask for, or `None` when none was given. A sheet starts from the
    /// print profile (its space is set by the core, so the layout name here is only a placeholder)
    /// and model space from the default profile.
    pub(super) fn view_profile(args: &ConvertArgs, sheet: bool) -> Option<ViewProfile> {
        let v = &args.view;
        if v.given().is_empty() {
            return None;
        }
        let rule = |word: &str| {
            if word == "draw" {
                LayerStateRule::Ignore
            } else {
                LayerStateRule::Exclude
            }
        };
        let mut p = if sheet {
            ViewProfile::print_profile("")
        } else {
            ViewProfile::default_profile()
        };
        if let Some(w) = &v.off_layers {
            p.layers.off = rule(w);
        }
        if let Some(w) = &v.frozen_layers {
            p.layers.frozen = rule(w);
        }
        if let Some(w) = &v.no_plot_layers {
            p.layers.no_plot = rule(w);
        }
        if let Some(w) = &v.frozen_text {
            p.layers.frozen_text = rule(w);
        }
        if let Some(w) = &v.no_plot_text {
            p.layers.no_plot_text = rule(w);
        }
        if let Some(w) = &v.invisible_entities {
            p.entities.invisible = rule(w);
        }
        if let Some(w) = &v.viewport_freezes {
            p.layers.frozen_in_viewport = if w == "apply" {
                ViewportFreezeRule::Apply
            } else {
                ViewportFreezeRule::IgnoreAndReport
            };
        }
        if let Some(w) = &v.insert_layer {
            p.layers.insert_override = if w == "own" {
                InsertLayerRule::OwnLayerOnly
            } else {
                InsertLayerRule::InheritOnLayerZero
            };
        }
        if let Some(n) = v.max_insert_depth {
            p.nested_inserts.max_depth = n;
        }
        p.layers.overrides.extend(
            v.layer_hidden
                .iter()
                .map(|n| (n.clone(), LayerOverride::Hidden)),
        );
        p.layers.overrides.extend(
            v.layer_visible
                .iter()
                .map(|n| (n.clone(), LayerOverride::Visible)),
        );
        Some(p)
    }

    /// The unit, the capabilities, the calibration, the view, the limits, the layout of the artifact,
    /// the component streams, the regions step and the thumbnail, the same for model space and a sheet.
    fn configure<'a>(
        c: PvecConversion<'a>,
        args: &ConvertArgs,
        unit: Option<SourceUnit>,
        per_metre: f64,
        calibration: Option<&Calibration>,
        sheet: bool,
    ) -> PvecConversion<'a> {
        // The profile replaces any earlier capability setting, so it goes first.
        let mut c = c;
        if let Some(p) = &args.capability.profile {
            c = c.profile(match p.as_str() {
                "strict" => ConversionProfile::Strict,
                "permissive" => ConversionProfile::Permissive,
                _ => ConversionProfile::Default,
            });
        }
        if !args.capability.allow_capability.is_empty() {
            c = c.allowed_capabilities(
                args.capability
                    .allow_capability
                    .iter()
                    .map(|&id| CapabilityId::new(id)),
            );
        }
        if let Some(p) = &args.capability.capability_policy {
            c = c.capability_policy(match p.as_str() {
                "reject-input" => CapabilityPolicy::RejectInput,
                "downgrade" => CapabilityPolicy::Downgrade,
                _ => CapabilityPolicy::RejectEntity,
            });
        }
        if let Some(cal) = calibration {
            c = c.calibration(cal.clone());
        }
        // The limits ride on the view profile, so a profile goes before them.
        if let Some(p) = view_profile(args, sheet) {
            c = c.view_profile(p);
        }
        if let Some(l) = source_limits(args) {
            c = c.limits(l);
        }
        if let Some(cell_capacity) = args.writer.leaf_capacity {
            c = c.leaves(LeafConfig {
                cell_capacity,
                tier0_max_error: args.writer.tier0_max_error,
            });
        }
        if let (Some(root), Some(budget)) = (&args.writer.spill_dir, args.writer.spill_budget) {
            c = c.spill(SpillConfig {
                root: root.clone(),
                budget,
            });
        }
        let c = c
            .components(if args.geometry_only {
                StreamSelection::NONE
            } else {
                StreamSelection::ALL
            })
            .annotations(args.annotations)
            .regions(args.regions.then(|| region_config(args, per_metre)));
        let c = if args.element_rules.is_empty() {
            c
        } else {
            c.element_rules(
                args.element_rules
                    .iter()
                    .fold(ElementRules::new(), |rules, r| {
                        let matcher = match &r.matcher {
                            Matcher::Layer(g) => RuleMatch::Layer(g.clone()),
                            Matcher::Block(g) => RuleMatch::Block(g.clone()),
                            Matcher::Colour(n) => RuleMatch::Colour(SourceColour::Aci(*n)),
                            Matcher::Lineweight(w) => RuleMatch::Lineweight(*w),
                        };
                        rules.with_rule(
                            matcher,
                            if r.wall {
                                ElementClass::Wall
                            } else {
                                ElementClass::Column
                            },
                        )
                    }),
            )
        };
        #[cfg(feature = "pvec-raster-thumbnail")]
        let c = match args.thumbnail {
            Some(width) => c.thumbnail(width),
            None => c,
        };
        match unit {
            Some(u) => c.unit(u),
            None => c,
        }
    }

    /// The PNG a conversion drew, beside its `.pvec` with the extension `.png`.
    #[cfg(feature = "pvec-raster-thumbnail")]
    fn save_thumbnail(artifact: &Path, png: Option<&[u8]>) -> Result<(), String> {
        let Some(png) = png else { return Ok(()) };
        let to = artifact.with_extension("png");
        std::fs::write(&to, png).map_err(|e| format!("cannot write {}: {e}", to.display()))
    }

    /// `viprs pvec layouts`.
    pub(super) fn layouts(args: &LayoutsArgs) {
        let sheets = match paper_sheets::<AcadrustProvider>(CadSource::Path(&args.drawing)) {
            Ok(sheets) => sheets,
            Err(e) => operational_error(&with_causes(&e)),
        };
        let skip = |s: &Option<SheetSkip>| match s {
            None => None,
            Some(SheetSkip::Empty) => Some("empty"),
            Some(SheetSkip::EmptyAfterView) => Some("empty-after-view"),
            Some(_) => Some("skipped"),
        };
        if args.json {
            let layouts: Vec<_> = sheets
                .iter()
                .map(|s| {
                    serde_json::json!({
                        "tab_order": s.tab_order,
                        "layout": s.layout,
                        "sheet": s.skip.is_none(),
                        "skip": skip(&s.skip),
                    })
                })
                .collect();
            println!("{}", serde_json::json!({ "v": 1, "layouts": layouts }));
        } else {
            for s in &sheets {
                println!(
                    "{:>3} {}  {}",
                    s.tab_order,
                    s.layout,
                    skip(&s.skip).unwrap_or("sheet")
                );
            }
        }
        if sheets.is_empty() {
            eprintln!(
                "{} has no paper layout; `viprs pvec convert --model` converts model space",
                args.drawing.display()
            );
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
        // What the calibration flags state, and a sidecar's id, are settled before anything is written.
        let calibration = match calibration(args) {
            Ok(c) => c,
            Err(e) => crate::usage_error(&e, "see `viprs pvec convert --help` for the scale flags"),
        };
        let sidecar = args.calibration.georef_sidecar.as_deref().and_then(|path| {
            // A file that cannot be read, or has a schema version this reader does not know,
            // stops the run before anything is converted.
            #[allow(deprecated)]
            let id = GeoreferenceHandoff::read_from(path).map(|h| {
                let GeoreferenceHandoff::V1(v1) = h;
                v1.calibration_id
            });
            match id {
                Ok(Some(_)) => calibration.clone().map(|c| (path, c)),
                Ok(None) => {
                    eprintln!(
                        "note: {} names no calibration id, so nothing is imported from it",
                        path.display()
                    );
                    None
                }
                Err(e) => operational_error(&format!("{}: {}", path.display(), with_causes(&e))),
            }
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
        if let Some(dir) = &args.writer.spill_dir
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            fail(&format!("cannot create {}: {e}", dir.display()));
        }
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
            match configure(c, args, unit, per_metre, calibration.as_ref(), false).convert() {
                Ok(out) => {
                    eprintln!("model  written  {} ({} entities)", name(&dest), out.written);
                    if let Some((path, cal)) = &sidecar {
                        eprintln!("model  {}", import(&dest, path, cal));
                    }
                    #[cfg(feature = "pvec-raster-thumbnail")]
                    if let Err(e) = save_thumbnail(&dest, out.thumbnail.as_deref()) {
                        fail(&e);
                    }
                }
                Err(PvecConvertError::Cancelled) => std::process::exit(EXIT_INTERRUPTED),
                Err(e) => fail(&explain(&e, unit.is_some())),
            }
            return;
        }

        let mut set = PvecLayoutSet::new(CadSource::Path(&args.drawing), &args.out_dir)
            .stem(stem)
            .sheets(move |c| {
                configure(
                    c.cancel(cancel.clone()),
                    args,
                    unit,
                    per_metre,
                    calibration.as_ref(),
                    true,
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
            #[cfg(feature = "pvec-raster-thumbnail")]
            if let Ok(o) = &s.outcome
                && let Err(e) = save_thumbnail(&s.file, o.thumbnail.as_deref())
            {
                fail(&e);
            }
            if let (Ok(_), Some((path, cal))) = (&s.outcome, &sidecar) {
                eprintln!("{:>3} {}", s.tab_order, import(&s.file, path, cal));
            }
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
    use super::ConvertArgs;
    use super::imp::{region_config, source_limits, units_per_metre, view_profile};
    use clap::Parser;
    use libviprs::pvec::{
        InsertLayerRule, LayerOverride, LayerStateRule, SourceLimits, SourceUnit, ViewProfile,
        ViewportFreezeRule,
    };

    /// `ConvertArgs` is an `Args`, so the cells parse it the way the binary does, behind a parser.
    #[derive(Parser)]
    struct Cli {
        #[command(flatten)]
        args: ConvertArgs,
    }

    fn parse(extra: &[&str]) -> ConvertArgs {
        let mut argv = vec!["viprs", "a.dwg", "out"];
        argv.extend_from_slice(extra);
        Cli::try_parse_from(argv).expect("the flags parse").args
    }

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

    /// The region knobs are metres: the defaults are the ones the step shipped with, and each flag
    /// lands in its own field, the lengths scaled to the drawing's unit and the others not.
    #[test]
    fn region_flags_land_in_their_own_fields() {
        let r = region_config(&parse(&["--regions"]), 1000.0);
        assert_eq!((r.gap_tolerance, r.corridor_max_width), (500.0, 2500.0));
        assert_eq!(
            (r.corridor_min_aspect, r.max_faces, r.keep_unlabelled),
            (3.0, 1000, false)
        );
        let r = region_config(
            &parse(&[
                "--regions",
                "--region-gap",
                "0.25",
                "--region-corridor-width",
                "1.5",
                "--region-corridor-aspect",
                "4",
                "--region-max-faces",
                "7",
                "--region-keep-unlabelled",
            ]),
            100.0,
        );
        assert_eq!((r.gap_tolerance, r.corridor_max_width), (25.0, 150.0));
        assert_eq!(
            (r.corridor_min_aspect, r.max_faces, r.keep_unlabelled),
            (4.0, 7, true)
        );
    }

    /// Every reader limit lands in its own `SourceLimits` field, and none given leaves the core's
    /// defaults alone (`None`, so the view profile's own limits stand). `--max-insert-grid` has no
    /// drawing that trips it (none holds a MINSERT), so this is where it is held.
    #[test]
    fn each_limit_flag_lands_in_its_own_field() {
        assert_eq!(source_limits(&parse(&[])), None);
        let l = source_limits(&parse(&[
            "--max-block-depth",
            "1",
            "--max-entities",
            "2",
            "--max-polyline-points",
            "3",
            "--max-spline-control-points",
            "4",
            "--max-spline-knots",
            "5",
            "--max-insert-grid",
            "6",
            "--max-expanded-instances",
            "7",
            "--max-alloc-bytes",
            "8",
            "--max-name-bytes",
            "9",
        ]))
        .expect("limits were given");
        assert_eq!(
            (
                l.max_block_depth,
                l.max_entities,
                l.max_polyline_vertices,
                l.max_spline_control_points,
                l.max_spline_knots,
                l.max_insert_grid,
                l.max_expanded_instances,
                l.max_allocation_bytes,
                l.max_name_bytes,
            ),
            (1, 2, 3, 4, 5, 6, 7, 8, 9)
        );
        // Only the one asked for moves.
        let mut want = SourceLimits::default();
        want.max_insert_grid = 6;
        assert_eq!(
            source_limits(&parse(&["--max-insert-grid", "6"])),
            Some(want)
        );
    }

    /// The view flags replace one rule each over the profile they start from: the default profile for
    /// model space and the print profile for a sheet. None given is `None`.
    #[test]
    fn view_flags_replace_one_rule_each_over_their_base() {
        assert_eq!(view_profile(&parse(&[]), false), None);
        assert_eq!(view_profile(&parse(&[]), true), None);
        let args = parse(&[
            "--off-layers",
            "draw",
            "--frozen-layers",
            "draw",
            "--no-plot-layers",
            "draw",
            "--frozen-text",
            "exclude",
            "--no-plot-text",
            "exclude",
            "--invisible-entities",
            "draw",
            "--viewport-freezes",
            "apply",
            "--insert-layer",
            "own",
            "--max-insert-depth",
            "5",
            "--layer-hidden",
            "A",
            "--layer-visible",
            "B",
        ]);
        for sheet in [false, true] {
            let p = view_profile(&args, sheet).expect("flags were given");
            let l = &p.layers;
            assert_eq!(
                (l.off, l.frozen, l.no_plot, l.frozen_text, l.no_plot_text),
                (
                    LayerStateRule::Ignore,
                    LayerStateRule::Ignore,
                    LayerStateRule::Ignore,
                    LayerStateRule::Exclude,
                    LayerStateRule::Exclude
                )
            );
            assert_eq!(p.entities.invisible, LayerStateRule::Ignore);
            assert_eq!(l.frozen_in_viewport, ViewportFreezeRule::Apply);
            assert_eq!(l.insert_override, InsertLayerRule::OwnLayerOnly);
            assert_eq!(p.nested_inserts.max_depth, 5);
            assert_eq!(
                l.overrides,
                [
                    ("A".to_owned(), LayerOverride::Hidden),
                    ("B".to_owned(), LayerOverride::Visible)
                ]
            );
        }
        // A rule not written stays the base's own: no-plot layers are drawn in model space and
        // left out of a sheet, and a viewport freeze is only applied on a sheet.
        let args = parse(&["--off-layers", "draw"]);
        let model = view_profile(&args, false).unwrap();
        let sheet = view_profile(&args, true).unwrap();
        assert_eq!(model.layers.no_plot, LayerStateRule::Ignore);
        assert_eq!(sheet.layers.no_plot, LayerStateRule::Exclude);
        assert_eq!(
            model.layers.frozen_in_viewport,
            ViewportFreezeRule::IgnoreAndReport
        );
        assert_eq!(sheet.layers.frozen_in_viewport, ViewportFreezeRule::Apply);
        assert_eq!(
            model.viewport_clipping,
            ViewProfile::default_profile().viewport_clipping
        );
    }
}
