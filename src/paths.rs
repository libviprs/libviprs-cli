//! Path comparisons shared by the commands that refuse to write over their own input.

/// Whether two paths name one file: the same path once the directory part is resolved (so a
/// detour, a symlink or a file that does not exist yet is seen through), or on Unix the same
/// device and inode (a hard link).
pub(crate) fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    let resolve = |p: &std::path::Path| {
        std::fs::canonicalize(p).ok().or_else(|| {
            let parent = p.parent().filter(|d| !d.as_os_str().is_empty());
            let dir = std::fs::canonicalize(parent.unwrap_or(std::path::Path::new("."))).ok()?;
            Some(dir.join(p.file_name()?))
        })
    };
    if resolve(a).zip(resolve(b)).is_some_and(|(x, y)| x == y) {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(x), Ok(y)) = (std::fs::metadata(a), std::fs::metadata(b)) {
            return (x.dev(), x.ino()) == (y.dev(), y.ino());
        }
    }
    false
}
