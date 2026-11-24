//! The flags of `viprs pvec convert` past the layout and stream choices (libviprs-cli#151): the page
//! and the image limits of a PDF, and for a DWG the profile and capability policy, the calibration
//! stated at convert time, the view profile, the reader limits, the leaf and spill settings and the
//! region tuning. They are plain clap groups: what they mean to the core is in `pvec_convert.rs`
//! (`imp`) and `pvec_convert/pdf.rs`. Each group can say which of its flags were given, so a flag
//! that belongs to the other kind of input is refused (exit 2) before anything is read.

use std::path::PathBuf;

use clap::{ArgGroup, Args};

use crate::ops::foreign::positive_finite;
use crate::pvec_read::{non_negative, ratio, segment};

/// The names of the flags in `set` that were given.
pub(super) fn given(set: &[(bool, &'static str)]) -> Vec<&'static str> {
    set.iter().filter(|(on, _)| *on).map(|(_, n)| *n).collect()
}

/// A PDF's page and what its images may cost.
#[derive(Args)]
#[command(next_help_heading = "PDF input")]
pub(super) struct PdfArgs {
    /// The 1-based page of a PDF to convert. A PDF is any input whose name ends in `.pdf`.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub(super) page: Option<u32>,

    /// Most images a PDF page may have. A page over it is refused (exit 1), naming `max_images`.
    /// 4096 by default.
    #[arg(long, value_name = "N", value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..))]
    pub(super) max_images: Option<usize>,

    /// Most encoded image bytes a PDF page may hold in all. A page over it is refused (exit 1),
    /// naming `max_image_bytes`. 512 MiB by default.
    #[arg(long, value_name = "BYTES", value_parser = clap::value_parser!(u64).range(1..))]
    pub(super) max_image_bytes: Option<u64>,

    /// Most encoded bytes one image stream may hold. An image over it is left out and the page still
    /// converts. 64 MiB by default.
    #[arg(long, value_name = "BYTES", value_parser = clap::value_parser!(u64).range(1..))]
    pub(super) max_image_stream_bytes: Option<u64>,

    /// Most pixels all the images of a page may have together and still be baked. Over it none is
    /// baked and the report says `max-image-pixels`. 2^30 by default.
    #[arg(long, value_name = "PIXELS", value_parser = clap::value_parser!(u64).range(1..))]
    pub(super) max_image_pixels: Option<u64>,

    /// Most pixels one image may have. A bigger one is refused in the report and the page still
    /// converts. 64 Mi pixels by default.
    #[arg(long, value_name = "PIXELS", value_parser = clap::value_parser!(u64).range(1..))]
    pub(super) max_pixels_per_image: Option<u64>,
}

impl PdfArgs {
    pub(super) fn given(&self) -> Vec<&'static str> {
        given(&[
            (self.page.is_some(), "--page"),
            (self.max_images.is_some(), "--max-images"),
            (self.max_image_bytes.is_some(), "--max-image-bytes"),
            (
                self.max_image_stream_bytes.is_some(),
                "--max-image-stream-bytes",
            ),
            (self.max_image_pixels.is_some(), "--max-image-pixels"),
            (
                self.max_pixels_per_image.is_some(),
                "--max-pixels-per-image",
            ),
        ])
    }
}

/// What a DWG conversion may use, and what happens to an entity that needs more.
#[derive(Args)]
#[command(next_help_heading = "Capabilities (DWG)")]
pub(super) struct CapabilityArgs {
    /// A named capability set and policy: `strict` (only exactly stored entities, and one that is not
    /// aborts the run), `default` (every capability the mapping names; an entity outside is left out and
    /// counted) or `permissive` (as default, with a downgrade tried first). The report names the one that ran.
    #[arg(long, value_name = "PROFILE", value_parser = ["strict", "default", "permissive"])]
    pub(super) profile: Option<String>,

    /// What happens to an entity that needs a capability outside the allowed set, in place of the
    /// profile's: `reject-input` (the run fails), `reject-entity` (it is left out and counted) or
    /// `downgrade` (a weaker form is tried first; none exists yet, so it is left out). The report calls
    /// the run `Custom`.
    #[arg(long, value_name = "POLICY", value_parser = ["reject-input", "reject-entity", "downgrade"])]
    pub(super) capability_policy: Option<String>,

    /// The PVec capability ids this run may use, in place of the profile's set (repeatable, or comma
    /// separated). The report lists them.
    #[arg(long, value_name = "ID", value_delimiter = ',')]
    pub(super) allow_capability: Vec<u32>,
}

impl CapabilityArgs {
    pub(super) fn given(&self) -> Vec<&'static str> {
        given(&[
            (self.profile.is_some(), "--profile"),
            (self.capability_policy.is_some(), "--capability-policy"),
            (!self.allow_capability.is_empty(), "--allow-capability"),
        ])
    }
}

/// The scale a DWG conversion writes into the artifact. A DWG gets none unless it is given here: its
/// unit is already the manifest's. The three ways of stating one are the ones `pvec calibrate` has.
#[derive(Args)]
#[command(
    next_help_heading = "Calibration (DWG)",
    group(ArgGroup::new("scale_mode").args(["scale", "plan_scale", "measured"]))
)]
pub(super) struct CalibrationArgs {
    /// What one drawing unit is in the world, in metres, stated by hand (provenance `manual`).
    #[arg(long, value_name = "METRES", allow_negative_numbers = true, value_parser = positive_finite)]
    pub(super) scale: Option<f64>,

    /// The plan's printed scale as two whole numbers, `1:100`; needs `--paper-unit`. One drawing
    /// unit is then the paper unit times 100, in metres (provenance `plan-scale`).
    #[arg(long, value_name = "N:D", requires = "paper_unit", value_parser = ratio)]
    pub(super) plan_scale: Option<(u32, u32)>,

    /// What one drawing unit is on paper, in metres (0.001 for a sheet drawn in millimetres).
    #[arg(
        long,
        value_name = "METRES",
        requires = "plan_scale",
        allow_negative_numbers = true,
        value_parser = positive_finite
    )]
    pub(super) paper_unit: Option<f64>,

    /// Two points in drawing units whose distance apart was measured, `X0,Y0,X1,Y1`; needs
    /// `--distance` (provenance `measured`).
    #[arg(
        long,
        value_name = "X0,Y0,X1,Y1",
        requires = "distance",
        allow_hyphen_values = true,
        value_parser = segment
    )]
    pub(super) measured: Option<[f64; 4]>,

    /// The measured distance between the two `--measured` points, in metres.
    #[arg(
        long,
        value_name = "METRES",
        requires = "measured",
        allow_negative_numbers = true,
        value_parser = positive_finite
    )]
    pub(super) distance: Option<f64>,

    /// A note stored with the scale (where it came from). Needs a scale.
    #[arg(long, value_name = "TEXT", requires = "scale_mode")]
    pub(super) scale_note: Option<String>,

    /// The relative uncertainty of the scale, 0 or more (0.01 is 1%). Needs a scale.
    #[arg(
        long,
        value_name = "RATIO",
        requires = "scale_mode",
        allow_negative_numbers = true,
        value_parser = non_negative
    )]
    pub(super) scale_uncertainty: Option<f64>,

    /// A retired `.georef.json` sidecar to bring in: its calibration id becomes the external id of the
    /// calibration the scale flags state, which is written into each artifact. A sidecar that names no id
    /// imports nothing. Needs a scale, since that is the calibration the id stands for.
    #[arg(long, value_name = "FILE", requires = "scale_mode")]
    pub(super) georef_sidecar: Option<PathBuf>,
}

impl CalibrationArgs {
    pub(super) fn given(&self) -> Vec<&'static str> {
        given(&[
            (self.scale.is_some(), "--scale"),
            (self.plan_scale.is_some(), "--plan-scale"),
            (self.paper_unit.is_some(), "--paper-unit"),
            (self.measured.is_some(), "--measured"),
            (self.distance.is_some(), "--distance"),
            (self.scale_note.is_some(), "--scale-note"),
            (self.scale_uncertainty.is_some(), "--scale-uncertainty"),
            (self.georef_sidecar.is_some(), "--georef-sidecar"),
        ])
    }
}

/// The view profile a DWG conversion draws under. A model-space conversion starts from the default
/// profile and a sheet from the print profile of its layout; each flag written here replaces one rule.
#[derive(Args)]
#[command(next_help_heading = "View profile (DWG)")]
pub(super) struct ViewArgs {
    /// Layers that are switched off: `exclude` (the default) or `draw`.
    #[arg(long, value_name = "RULE", value_parser = ["exclude", "draw"])]
    pub(super) off_layers: Option<String>,

    /// Frozen layers: `exclude` (the default) or `draw`.
    #[arg(long, value_name = "RULE", value_parser = ["exclude", "draw"])]
    pub(super) frozen_layers: Option<String>,

    /// No-plot layers: `draw` for model space, `exclude` for a sheet, by default.
    #[arg(long, value_name = "RULE", value_parser = ["exclude", "draw"])]
    pub(super) no_plot_layers: Option<String>,

    /// Text on a frozen layer, in place of `--frozen-layers` for it: `draw` by default.
    #[arg(long, value_name = "RULE", value_parser = ["exclude", "draw"])]
    pub(super) frozen_text: Option<String>,

    /// Text on a no-plot layer, in place of `--no-plot-layers` for it: `draw` for model space, `exclude`
    /// for a sheet, by default.
    #[arg(long, value_name = "RULE", value_parser = ["exclude", "draw"])]
    pub(super) no_plot_text: Option<String>,

    /// Entities whose own invisible flag is set: `exclude` (the default) or `draw`.
    #[arg(long, value_name = "RULE", value_parser = ["exclude", "draw"])]
    pub(super) invisible_entities: Option<String>,

    /// A layer frozen only inside a viewport: `report` (draw it and say a freeze was ignored; the model
    /// default) or `apply` (the sheet default, and the core refuses it for model space).
    #[arg(long, value_name = "RULE", value_parser = ["report", "apply"])]
    pub(super) viewport_freezes: Option<String>,

    /// Whether layer 0 inside an INSERT takes the INSERT's layer: `inherit` (the default) or `own`.
    #[arg(long, value_name = "RULE", value_parser = ["inherit", "own"])]
    pub(super) insert_layer: Option<String>,

    /// Deepest INSERT chain followed. 64 by default.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub(super) max_insert_depth: Option<u32>,

    /// Force a layer hidden, whatever its flags say (repeatable, name matched with the case ignored).
    #[arg(long, value_name = "LAYER")]
    pub(super) layer_hidden: Vec<String>,

    /// Force a layer visible, whatever its flags say (repeatable, name matched with the case ignored).
    #[arg(long, value_name = "LAYER")]
    pub(super) layer_visible: Vec<String>,
}

impl ViewArgs {
    pub(super) fn given(&self) -> Vec<&'static str> {
        given(&[
            (self.off_layers.is_some(), "--off-layers"),
            (self.frozen_layers.is_some(), "--frozen-layers"),
            (self.no_plot_layers.is_some(), "--no-plot-layers"),
            (self.frozen_text.is_some(), "--frozen-text"),
            (self.no_plot_text.is_some(), "--no-plot-text"),
            (self.invisible_entities.is_some(), "--invisible-entities"),
            (self.viewport_freezes.is_some(), "--viewport-freezes"),
            (self.insert_layer.is_some(), "--insert-layer"),
            (self.max_insert_depth.is_some(), "--max-insert-depth"),
            (!self.layer_hidden.is_empty(), "--layer-hidden"),
            (!self.layer_visible.is_empty(), "--layer-visible"),
        ])
    }
}

/// The ceilings of the DWG reader, one flag each, named as `viprs vector` names them.
#[derive(Args)]
#[command(next_help_heading = "Reader limits (DWG)")]
pub(super) struct SourceLimitArgs {
    /// Deepest INSERT nesting. 64 by default.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub(super) max_block_depth: Option<u32>,

    /// Most entities looked at, nested expansion included. 20 million by default.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pub(super) max_entities: Option<u64>,

    /// Most points in one polyline. 2^20 by default.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pub(super) max_polyline_points: Option<u64>,

    /// Most control points in one spline. 2^20 by default.
    #[arg(long, value_name = "N", value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..))]
    pub(super) max_spline_control_points: Option<usize>,

    /// Longest knot vector (and weight array) of one spline. 2^20 + 64 by default.
    #[arg(long, value_name = "N", value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..))]
    pub(super) max_spline_knots: Option<usize>,

    /// Most instances one INSERT may place (MINSERT rows times columns). A million by default.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pub(super) max_insert_grid: Option<u64>,

    /// Most block instances expanded in total, multiplied through nesting. Ten million by default.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u64).range(1..))]
    pub(super) max_expanded_instances: Option<u64>,

    /// Estimated bytes of entity data held or produced. 2 GiB by default.
    #[arg(long, value_name = "BYTES", value_parser = clap::value_parser!(u64).range(1..))]
    pub(super) max_alloc_bytes: Option<u64>,

    /// Longest layer, block or class name, in bytes. 1020 by default.
    #[arg(long, value_name = "BYTES", value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..))]
    pub(super) max_name_bytes: Option<usize>,
}

impl SourceLimitArgs {
    pub(super) fn given(&self) -> Vec<&'static str> {
        given(&[
            (self.max_block_depth.is_some(), "--max-block-depth"),
            (self.max_entities.is_some(), "--max-entities"),
            (self.max_polyline_points.is_some(), "--max-polyline-points"),
            (
                self.max_spline_control_points.is_some(),
                "--max-spline-control-points",
            ),
            (self.max_spline_knots.is_some(), "--max-spline-knots"),
            (self.max_insert_grid.is_some(), "--max-insert-grid"),
            (
                self.max_expanded_instances.is_some(),
                "--max-expanded-instances",
            ),
            (self.max_alloc_bytes.is_some(), "--max-alloc-bytes"),
            (self.max_name_bytes.is_some(), "--max-name-bytes"),
        ])
    }
}

/// How the artifact is laid out and written: PVec's spatial directory, and the spill writer.
#[derive(Args)]
#[command(next_help_heading = "Layout and memory (DWG)")]
pub(super) struct WriterArgs {
    /// Lay PVec's spatial directory into the artifact, with this many entities to a leaf. Without it
    /// the artifact is a plain file with no spatial index.
    #[arg(long, value_name = "N", value_parser = clap::value_parser!(u32).range(1..))]
    pub(super) leaf_capacity: Option<u32>,

    /// Also write Tier 0 preview blocks, bounded to this error in drawing units. Needs
    /// `--leaf-capacity`; without it no Tier 0 block is written.
    #[arg(
        long,
        value_name = "UNITS",
        requires = "leaf_capacity",
        allow_negative_numbers = true,
        value_parser = positive_finite
    )]
    pub(super) tier0_max_error: Option<f64>,

    /// Keep the writer's pushes in files under this directory (created when it does not exist), and
    /// its memory inside `--spill-budget`. The artifact is the same bytes as without it. The memory
    /// is bounded when `--leaf-capacity` is set too; a plain artifact holds one block per geometry type.
    #[arg(long, value_name = "DIR", requires = "spill_budget")]
    pub(super) spill_dir: Option<PathBuf>,

    /// The writer's memory budget in bytes, before it spills into `--spill-dir`.
    #[arg(
        long,
        value_name = "BYTES",
        requires = "spill_dir",
        value_parser = clap::value_parser!(u64).range(1..)
    )]
    pub(super) spill_budget: Option<u64>,
}

impl WriterArgs {
    pub(super) fn given(&self) -> Vec<&'static str> {
        given(&[
            (self.leaf_capacity.is_some(), "--leaf-capacity"),
            (self.tier0_max_error.is_some(), "--tier0-max-error"),
            (self.spill_dir.is_some(), "--spill-dir"),
            (self.spill_budget.is_some(), "--spill-budget"),
        ])
    }
}

/// The knobs of the regions step, which they need.
#[derive(Args)]
#[command(next_help_heading = "Regions (DWG)")]
pub(super) struct RegionArgs {
    /// The widest gap between two wall ends that is closed, in metres. 0.5 by default. Needs `--regions`.
    #[arg(
        long,
        value_name = "METRES",
        requires = "regions",
        allow_negative_numbers = true,
        value_parser = positive_finite
    )]
    pub(super) region_gap: Option<f64>,

    /// A face whose short side is at most this is a corridor candidate, in metres. 2.5 by default.
    /// Needs `--regions`.
    #[arg(
        long,
        value_name = "METRES",
        requires = "regions",
        allow_negative_numbers = true,
        value_parser = positive_finite
    )]
    pub(super) region_corridor_width: Option<f64>,

    /// ... and whose long side is at least this many times the short one. 3 by default. Needs
    /// `--regions`.
    #[arg(
        long,
        value_name = "RATIO",
        requires = "regions",
        allow_negative_numbers = true,
        value_parser = positive_finite
    )]
    pub(super) region_corridor_aspect: Option<f64>,

    /// The most faces the step builds before it refuses and the conversion carries on without regions.
    /// 1000 by default. Needs `--regions`.
    #[arg(
        long,
        value_name = "N",
        requires = "regions",
        value_parser = clap::builder::RangedU64ValueParser::<usize>::new().range(1..)
    )]
    pub(super) region_max_faces: Option<usize>,

    /// Keep faces no rule labels, as regions with no kind. Off omits them. Needs `--regions`.
    #[arg(long, requires = "regions")]
    pub(super) region_keep_unlabelled: bool,
}

impl RegionArgs {
    pub(super) fn given(&self) -> Vec<&'static str> {
        given(&[
            (self.region_gap.is_some(), "--region-gap"),
            (
                self.region_corridor_width.is_some(),
                "--region-corridor-width",
            ),
            (
                self.region_corridor_aspect.is_some(),
                "--region-corridor-aspect",
            ),
            (self.region_max_faces.is_some(), "--region-max-faces"),
            (self.region_keep_unlabelled, "--region-keep-unlabelled"),
        ])
    }
}
