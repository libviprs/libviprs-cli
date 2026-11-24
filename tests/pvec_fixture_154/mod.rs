//! The drawing the libviprs-cli#154 (CV2.5) tests share: four entities of different kinds in
//! source units, written by PVec's own writer so the file is not made by the code under test.
//!
//! * 0: a polyline, (0,0) (400,0) (400,300)
//! * 1: a circle, centre (1000,1000), radius 200
//! * 2: a polyline, (3000,3000) (3500,3400)
//! * 3: a clamped quadratic B-spline over (0,2000) (500,2600) (1000,2000) (1500,2600)

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

#[cfg(feature = "pvec-raster")]
#[allow(unused_imports)]
pub use drawing_parts::*;

pub fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_viprs"))
        .args(args)
        .output()
        .expect("viprs must spawn")
}

pub fn code(out: &Output) -> i32 {
    out.status.code().expect("the process must exit normally")
}

pub fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

pub fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

pub fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viprs-cv25-154-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[track_caller]
pub fn ok(args: &[&str]) -> Output {
    let out = run(args);
    assert_eq!(code(&out), 0, "{args:?}:\n{}", stderr(&out));
    out
}

pub fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("stdout is not one JSON document ({e}):\n{}", stdout(out)))
}

/// Exit 2 with every needle on stderr, and nothing written beside the input.
#[track_caller]
pub fn usage(args: &[&str], needles: &[&str]) {
    let out = run(args);
    let err = stderr(&out);
    assert_eq!(code(&out), 2, "{args:?}:\n{err}");
    for needle in needles {
        assert!(err.contains(needle), "{args:?}: {needle:?} not in:\n{err}");
    }
}

/// A file the usage refusals never get far enough to read.
pub fn placeholder(dir: &Path) -> PathBuf {
    let path = dir.join("placeholder.pvec");
    std::fs::write(&path, b"never opened").unwrap();
    path
}

/// A file that is not a `.pvec` at all.
pub fn not_a_pvec(dir: &Path) -> PathBuf {
    let path = dir.join("not.pvec");
    std::fs::write(&path, b"this is not a pvec file").unwrap();
    path
}

#[cfg(feature = "pvec-raster")]
mod drawing_parts {
    use std::path::{Path, PathBuf};

    use pvec_facade::{Bounds2, Curve, EntityId, Manifest, PvecReader, PvecWriter};
    use pvec_geometry::{BSpline, Circle, Placement2, Point2, Polyline, Vector2};

    pub fn id(n: u32) -> EntityId {
        EntityId::new(n).unwrap()
    }

    pub fn point(x: f64, y: f64) -> Point2 {
        Point2 { x, y }
    }

    /// The four curves, in entity order.
    pub fn curves() -> Vec<(EntityId, Curve)> {
        let poly = |pts: &[(f64, f64)]| {
            Curve::Polyline(Polyline::new(pts.iter().map(|&(x, y)| point(x, y)).collect()).unwrap())
        };
        let circle = Circle::new(
            Placement2::new(point(1000.0, 1000.0), Vector2::new(1.0, 0.0)).unwrap(),
            200.0,
        )
        .unwrap();
        let spline = BSpline::new(
            2,
            vec![
                point(0.0, 2000.0),
                point(500.0, 2600.0),
                point(1000.0, 2000.0),
                point(1500.0, 2600.0),
            ],
            vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0],
        )
        .unwrap();
        vec![
            (id(0), poly(&[(0.0, 0.0), (400.0, 0.0), (400.0, 300.0)])),
            (id(1), Curve::Circle(circle)),
            (id(2), poly(&[(3000.0, 3000.0), (3500.0, 3400.0)])),
            (id(3), Curve::BSpline(spline)),
        ]
    }

    /// The drawing, written to `dir`.
    pub fn drawing(dir: &Path) -> PathBuf {
        let mut w = PvecWriter::new(Manifest {
            metres_per_unit: 0.001,
            bounds: Bounds2 {
                min: [0.0, 0.0],
                max: [4000.0, 4000.0],
            },
            capabilities: Vec::new(),
            producer_id: "viprs-cli-154".into(),
            producer_version: "1".into(),
        })
        .unwrap();
        for (entity, curve) in curves() {
            w.push(entity, &curve).unwrap();
        }
        let path = dir.join("drawing.pvec");
        std::fs::write(&path, w.finish().unwrap()).unwrap();
        path
    }

    pub fn open(path: &Path) -> PvecReader {
        PvecReader::open(std::fs::read(path).unwrap()).expect("PVec's reader must open the file")
    }
}
