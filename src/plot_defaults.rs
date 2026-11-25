//! `viprs plot-defaults` (libviprs-cli#162, step 7 of the plan on libviprs#1670): write the built-in plot style
//! table and plotter config out as real files, store the user's own pair for later plots, show what a plot
//! would use and where each file comes from, and clear the stored pair. The same walk decides what
//! `viprs plot` uses.
//!
//! ```text
//! viprs plot-defaults write DIR [--force]         the built-in .ctb and .pc3, as files, in DIR
//! viprs plot-defaults set [--ctb F] [--pc3 F]     store copies as the default under the config directory
//! viprs plot-defaults show [--json]               the directory, and per file the level it comes from
//! viprs plot-defaults clear [--ctb] [--pc3]       remove the stored default (both without a flag)
//! ```
//!
//! The precedence is walked per file and independently: the flag (`--ctb`, `--pc3`), then the variable
//! (`LIBVIPRS_PLOT_CTB`, `LIBVIPRS_PLOT_PC3`), then the stored default, then the built-in. `--no-defaults`
//! skips the variable and the stored default and never the flag; `--ctb none` is its own answer for the
//! table (the objects' own colours and widths). The core has two levels of its own (what the caller set,
//! else the built-in); this is the caller that walks the other two (libviprs#1670, decisions 5 to 8).
//!
//! The config directory is the core's: `LIBVIPRS_CONFIG_DIR` when absolute, else `$XDG_CONFIG_HOME/libviprs`
//! or `~/.config/libviprs` (Linux), `~/Library/Application Support/libviprs` (macOS) or
//! `%APPDATA%\libviprs` (Windows), a relative value ignored. Nothing here creates it unless `set` is asked
//! to, and a plot never does.
//!
//! # Exit codes
//!
//! 0 when it did what it was asked, 1 when a file was refused (with the core's code), a variable names a
//! file that is not there, a stored default no longer loads (`plot-default-unreadable`), the config directory
//! is not usable or the build lacks `pvec-convert`, and 2 for a usage mistake.

use std::path::PathBuf;

use clap::{Args, Subcommand};

use crate::usage_error;

/// `viprs plot-defaults`.
#[derive(Args)]
pub(crate) struct PlotDefaultsArgs {
    #[command(subcommand)]
    verb: Verb,
}

#[derive(Subcommand)]
enum Verb {
    /// Write the built-in plot style table and PDF plotter config into DIR as libviprs-monochrome.ctb and
    /// libviprs-pdf.pc3, real files AutoCAD can open. DIR is created when it does not exist. A file
    /// already there is kept unless --force, and a symlink is never written through.
    Write {
        /// The directory the two files go into.
        dir: PathBuf,
        /// Replace a regular file that is already there.
        #[arg(long)]
        force: bool,
    },
    /// Store copies of a .ctb and a .pc3 as the default for later plots, under the config directory
    /// (plot/default.ctb and plot/default.pc3, private to the user). Each is checked the way a plot
    /// reads it, and nothing is stored when either is refused.
    Set {
        /// The plot style table (a .ctb) to store.
        #[arg(long, value_name = "FILE")]
        ctb: Option<PathBuf>,
        /// The plotter config (a .pc3, JSON) to store.
        #[arg(long, value_name = "FILE")]
        pc3: Option<PathBuf>,
    },
    /// Print the config directory and, per file, where a plot would take it from: the flag, the
    /// variable, the stored default or the built-in. The flags are the ones viprs plot has.
    Show {
        /// One JSON object instead of text.
        #[arg(long)]
        json: bool,
        /// Show the walk as if plotted with this table (none for the objects' own colours).
        #[arg(long, value_name = "FILE")]
        ctb: Option<PathBuf>,
        /// Show the walk as if plotted with this plotter config.
        #[arg(long, value_name = "FILE")]
        pc3: Option<PathBuf>,
        /// Skip the variables and the stored default.
        #[arg(long)]
        no_defaults: bool,
    },
    /// Remove the stored default: both files, or only the one named.
    Clear {
        /// Only the stored plot style table.
        #[arg(long)]
        ctb: bool,
        /// Only the stored plotter config.
        #[arg(long)]
        pc3: bool,
    },
}

pub(crate) fn run(args: PlotDefaultsArgs) {
    if let Verb::Set {
        ctb: None,
        pc3: None,
    } = &args.verb
    {
        usage_error(
            "set needs a file to store",
            "give --ctb FILE, --pc3 FILE or both",
        );
    }
    #[cfg(feature = "pvec-convert")]
    imp::run(args.verb);
    #[cfg(not(feature = "pvec-convert"))]
    {
        let _ = args;
        crate::operational_error(
            "`viprs plot-defaults` is not in this build; rebuild with `--features pvec-convert`",
        );
    }
}

/// Where a plot file comes from, in the order the walk tries them.
#[cfg(feature = "pvec-convert")]
pub(crate) enum Source {
    /// `--ctb FILE` or `--pc3 FILE`.
    Flag(PathBuf),
    /// `LIBVIPRS_PLOT_CTB` or `LIBVIPRS_PLOT_PC3`: the variable's name and the file it names.
    Env(&'static str, PathBuf),
    /// The stored default.
    Stored(PathBuf),
    /// The built-in file the core carries.
    BuiltIn,
    /// `--ctb none`: no table, the objects' own colours and widths.
    None,
}

/// A file the walk settled on, and its bytes (none for the built-in and `none`).
#[cfg(feature = "pvec-convert")]
pub(crate) struct Pick {
    pub(crate) source: Source,
    bytes: Vec<u8>,
}

#[cfg(feature = "pvec-convert")]
pub(crate) use imp::{describe, device, table, walk};

#[cfg(feature = "pvec-convert")]
mod imp {
    use super::*;

    use std::fs;
    use std::io::{ErrorKind, Read, Write};
    use std::path::Path;

    use libviprs::cad::plot_defaults::{self, PlotDefaultFile};
    use libviprs::cad::plot_files::{
        MAX_PLOT_FILE_BYTES, Pc3File, PlotDevice, PlotStyleTable, default_pc3, default_plot_style,
        load_pc3, load_plot_style,
    };

    use crate::operational_error;

    /// The usual place for the defaults, from the real environment: `None` when no absolute directory
    /// can be worked out.
    fn config_dir() -> Option<PathBuf> {
        plot_defaults::config_dir(std::env::consts::OS, |name| std::env::var(name).ok())
    }

    /// The config directory, or the exit that says how to name one.
    fn config_dir_or_exit() -> PathBuf {
        config_dir().unwrap_or_else(|| {
            operational_error(
                "no usable config directory: set LIBVIPRS_CONFIG_DIR to an absolute path",
            )
        })
    }

    /// The name the core stores a default under, inside `plot/`.
    fn stored_name(which: PlotDefaultFile) -> &'static str {
        match which {
            PlotDefaultFile::Ctb => "default.ctb",
            PlotDefaultFile::Pc3 => "default.pc3",
        }
    }

    fn variable(which: PlotDefaultFile) -> &'static str {
        match which {
            PlotDefaultFile::Ctb => "LIBVIPRS_PLOT_CTB",
            PlotDefaultFile::Pc3 => "LIBVIPRS_PLOT_PC3",
        }
    }

    /// One line for a source, as `show` and `plot` both print it.
    pub(crate) fn describe(which: PlotDefaultFile, source: &Source) -> String {
        match (source, which) {
            (Source::Flag(p), PlotDefaultFile::Ctb) => format!("--ctb {}", p.display()),
            (Source::Flag(p), PlotDefaultFile::Pc3) => format!("--pc3 {}", p.display()),
            (Source::Env(var, p), _) => format!("{var} {}", p.display()),
            (Source::Stored(p), _) => format!("stored default {}", p.display()),
            (Source::BuiltIn, PlotDefaultFile::Ctb) => "built-in monochrome".to_owned(),
            (Source::BuiltIn, PlotDefaultFile::Pc3) => "built-in PDF".to_owned(),
            (Source::None, _) => "none, the objects' own colours and widths".to_owned(),
        }
    }

    /// A plot file's bytes, read through a cap one byte past the core's, so the loader refuses an
    /// oversize file by its own rule and a huge one is never read whole.
    fn read_plot_file(path: &Path, label: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        let read = fs::File::open(path).and_then(|f| {
            f.take(MAX_PLOT_FILE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
        });
        if let Err(e) = read {
            operational_error(&format!("{label}: cannot read {}: {e}", path.display()));
        }
        bytes
    }

    /// The level one file comes from. The flag first, then (unless `no_defaults`) the variable, then the
    /// stored default, then the built-in. A file a level names that cannot be read is an error and the
    /// walk does not go on, so a typo never falls back to something else.
    fn pick(which: PlotDefaultFile, flag: Option<&Path>, no_defaults: bool) -> Pick {
        let read = |source: Source| {
            let path = match &source {
                Source::Flag(p) | Source::Env(_, p) | Source::Stored(p) => p.clone(),
                Source::BuiltIn | Source::None => unreachable!("only a file is read"),
            };
            let bytes = read_plot_file(&path, &describe(which, &source));
            Pick { source, bytes }
        };
        if let Some(path) = flag {
            if which == PlotDefaultFile::Ctb && path == Path::new("none") {
                return Pick {
                    source: Source::None,
                    bytes: Vec::new(),
                };
            }
            return read(Source::Flag(path.to_owned()));
        }
        if !no_defaults {
            let var = variable(which);
            if let Some(value) = std::env::var_os(var).filter(|v| !v.is_empty()) {
                return read(Source::Env(var, PathBuf::from(value)));
            }
            if let Some(dir) = config_dir() {
                match plot_defaults::load(&dir, which) {
                    Ok(Some(bytes)) => {
                        let path = dir.join("plot").join(stored_name(which));
                        return Pick {
                            source: Source::Stored(path),
                            bytes,
                        };
                    }
                    Ok(None) => {}
                    Err(e) => operational_error(&format!("{e} ({})", e.code())),
                }
            }
        }
        Pick {
            source: Source::BuiltIn,
            bytes: Vec::new(),
        }
    }

    /// Walks both files: the table, then the plotter config.
    pub(crate) fn walk(ctb: Option<&Path>, pc3: Option<&Path>, no_defaults: bool) -> (Pick, Pick) {
        (
            pick(PlotDefaultFile::Ctb, ctb, no_defaults),
            pick(PlotDefaultFile::Pc3, pc3, no_defaults),
        )
    }

    /// The table the pick stands for, through the core's loader. `None` is the built-in, which the core
    /// applies itself when it is handed no table; `--ctb none` is the identity table.
    pub(crate) fn table(pick: &Pick) -> Option<PlotStyleTable> {
        let label = describe(PlotDefaultFile::Ctb, &pick.source);
        match &pick.source {
            Source::BuiltIn => None,
            Source::None => Some(PlotStyleTable::identity("none.ctb")),
            Source::Flag(p) | Source::Env(_, p) | Source::Stored(p) => {
                let name = p
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                Some(
                    load_plot_style(&name, &pick.bytes).unwrap_or_else(|e| {
                        operational_error(&format!("{label}: {e} ({})", e.code()))
                    }),
                )
            }
        }
    }

    /// The device facts the pick stands for, through the core's loader: the built-in PDF device for the
    /// built-in file. A binary PC3 is read and kept whole by the core but has no media to read, so it
    /// fills nothing and says so.
    pub(crate) fn device(pick: &Pick) -> Option<PlotDevice> {
        let label = describe(PlotDefaultFile::Pc3, &pick.source);
        match &pick.source {
            Source::BuiltIn => {
                Some(PlotDevice::from_pc3(&default_pc3()).expect("the built-in PC3 names a device"))
            }
            Source::None => None,
            Source::Flag(_) | Source::Env(..) | Source::Stored(_) => match load_pc3(&pick.bytes) {
                Ok(Pc3File::Json(pc3)) => Some(PlotDevice::from_pc3(&pc3).unwrap_or_else(|e| {
                    operational_error(&format!("{label}: {e} ({})", e.code()))
                })),
                Ok(_) => {
                    eprintln!(
                        "note: {label} is a binary PC3, which the core keeps but does not read the paper from, so it fills nothing"
                    );
                    None
                }
                Err(e) => operational_error(&format!("{label}: {e} ({})", e.code())),
            },
        }
    }

    pub(super) fn run(verb: Verb) {
        match verb {
            Verb::Write { dir, force } => write(&dir, force),
            Verb::Set { ctb, pc3 } => set(ctb.as_deref(), pc3.as_deref()),
            Verb::Show {
                json,
                ctb,
                pc3,
                no_defaults,
            } => show(json, ctb.as_deref(), pc3.as_deref(), no_defaults),
            Verb::Clear { ctb, pc3 } => clear(ctb, pc3),
        }
    }

    /// `write DIR`: both files are checked first, so a refusal writes neither, and each goes to a name of
    /// its own and is renamed into place.
    fn write(dir: &Path, force: bool) {
        let files = [
            (
                "libviprs-monochrome.ctb",
                default_plot_style().to_bytes().unwrap_or_else(|e| {
                    operational_error(&format!("cannot build the built-in table: {e:?}"))
                }),
            ),
            ("libviprs-pdf.pc3", default_pc3().to_bytes()),
        ];
        if let Err(e) = fs::create_dir_all(dir) {
            operational_error(&format!("cannot create {}: {e}", dir.display()));
        }
        for (name, _) in &files {
            let target = dir.join(name);
            match fs::symlink_metadata(&target) {
                Ok(m) if m.file_type().is_symlink() => operational_error(&format!(
                    "{} is a symlink, and a default is never written through one",
                    target.display()
                )),
                Ok(m) if !m.file_type().is_file() => {
                    operational_error(&format!("{} is not a regular file", target.display()))
                }
                Ok(_) if !force => operational_error(&format!(
                    "{} already exists; give --force to replace it",
                    target.display()
                )),
                Ok(_) => {}
                Err(e) if e.kind() == ErrorKind::NotFound => {}
                Err(e) => operational_error(&format!("{}: {e}", target.display())),
            }
        }
        for (name, bytes) in &files {
            let target = dir.join(name);
            let scratch = dir.join(format!(".{name}.{}.tmp", std::process::id()));
            let written = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&scratch)
                .and_then(|mut f| f.write_all(bytes).and_then(|()| f.sync_all()))
                .and_then(|()| fs::rename(&scratch, &target));
            if let Err(e) = written {
                let _ = fs::remove_file(&scratch);
                operational_error(&format!("cannot write {}: {e}", target.display()));
            }
            eprintln!("written  {}", target.display());
        }
    }

    /// `set`: both files are read and run through the core's checks in a scratch directory first, so a
    /// refusal of either stores neither.
    fn set(ctb: Option<&Path>, pc3: Option<&Path>) {
        let dir = config_dir_or_exit();
        let items: Vec<(PlotDefaultFile, &Path, Vec<u8>)> = [
            (PlotDefaultFile::Ctb, ctb, "--ctb"),
            (PlotDefaultFile::Pc3, pc3, "--pc3"),
        ]
        .into_iter()
        .filter_map(|(which, path, flag)| {
            path.map(|p| {
                (
                    which,
                    p,
                    read_plot_file(p, &format!("{flag} {}", p.display())),
                )
            })
        })
        .collect();
        let check =
            std::env::temp_dir().join(format!("viprs-plot-defaults-{}", std::process::id()));
        if let Err(e) = fs::create_dir(&check) {
            operational_error(&format!("cannot make {}: {e}", check.display()));
        }
        let refused = items.iter().find_map(|(which, path, bytes)| {
            plot_defaults::store(&check, *which, bytes)
                .err()
                .map(|e| format!("{}: {e} ({})", path.display(), e.code()))
        });
        let _ = fs::remove_dir_all(&check);
        if let Some(message) = refused {
            operational_error(&message);
        }
        for (which, path, bytes) in &items {
            if let Err(e) = plot_defaults::store(&dir, *which, bytes) {
                operational_error(&format!("{e} ({})", e.code()));
            }
            eprintln!(
                "stored  {}  as  {}",
                path.display(),
                dir.join("plot").join(stored_name(*which)).display()
            );
        }
    }

    /// `show`: the walk, with each file loaded the way a plot would load it.
    fn show(json: bool, ctb: Option<&Path>, pc3: Option<&Path>, no_defaults: bool) {
        let (ctb_pick, pc3_pick) = walk(ctb, pc3, no_defaults);
        let _ = table(&ctb_pick);
        let _ = device(&pc3_pick);
        let dir = config_dir();
        let name = |s: &Source| match s {
            Source::Flag(_) => "flag",
            Source::Env(..) => "env",
            Source::Stored(_) => "stored",
            Source::BuiltIn => "built-in",
            Source::None => "none",
        };
        let path = |s: &Source| match s {
            Source::Flag(p) | Source::Env(_, p) | Source::Stored(p) => {
                serde_json::Value::from(p.to_string_lossy().into_owned())
            }
            Source::BuiltIn | Source::None => serde_json::Value::Null,
        };
        if json {
            let body = serde_json::json!({
                "config_dir": dir.as_ref().map(|d| d.to_string_lossy().into_owned()),
                "ctb": { "source": name(&ctb_pick.source), "path": path(&ctb_pick.source) },
                "pc3": { "source": name(&pc3_pick.source), "path": path(&pc3_pick.source) },
            });
            println!("{body:#}");
            return;
        }
        match dir {
            Some(d) => println!("config directory: {}", d.display()),
            None => println!(
                "config directory: none usable (set LIBVIPRS_CONFIG_DIR to an absolute path)"
            ),
        }
        println!(
            "plot style table: {}",
            describe(PlotDefaultFile::Ctb, &ctb_pick.source)
        );
        println!(
            "plotter config: {}",
            describe(PlotDefaultFile::Pc3, &pc3_pick.source)
        );
    }

    /// `clear`: what was stored goes, what the user gave to `set` stays.
    fn clear(ctb: bool, pc3: bool) {
        let dir = config_dir_or_exit();
        let which: Vec<PlotDefaultFile> = match (ctb, pc3) {
            (true, false) => vec![PlotDefaultFile::Ctb],
            (false, true) => vec![PlotDefaultFile::Pc3],
            _ => vec![PlotDefaultFile::Ctb, PlotDefaultFile::Pc3],
        };
        let mut removed = 0;
        for w in which {
            match plot_defaults::clear(&dir, w) {
                Ok(true) => {
                    removed += 1;
                    eprintln!(
                        "cleared  {}",
                        dir.join("plot").join(stored_name(w)).display()
                    );
                }
                Ok(false) => {}
                Err(e) => operational_error(&format!("{e} ({})", e.code())),
            }
        }
        if removed == 0 {
            eprintln!("nothing stored under {}", dir.display());
        }
    }
}
