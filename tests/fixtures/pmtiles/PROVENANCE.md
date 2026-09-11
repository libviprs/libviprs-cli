# Two PMTiles archives this repository did not write

Both files here were produced by `go-pmtiles` v1.31.2 (source commit
`a3e4951ea6a0477b784c27c1dcbfd9c130878c5a`), the reference implementation of the
format, and neither has ever been through any libviprs code. That is the whole
point of keeping them: `viprs pmtiles verify` and `viprs pmtiles extract` walk
directories that this project's own writer produced, so testing them against our
own output can pass on a misreading of the spec that our writer and our reader
share. These two cannot.

| Archive | Bytes | sha256 |
|---|---|---|
| `dupes-z0z3.pmtiles` | 5007 | `bfc9db4c6ce6a04194e02b3d4815814adb05209f1aaba8591e4e1332f6e56a27` |
| `leaves-z0z7.pmtiles` | 869 | `fe5c9636be61abc60046d7f13837f8a3efb20ce3c38303644dac0cbec8248b8d` |

Copied from `.epicF/oracle/goldens/`, which carries the full provenance: how the
binary was pinned and verified, and how the archives were built (a hand-written
MBTiles converted with `pmtiles convert`, so the tile payloads are ours and the
archive layout is entirely go-pmtiles'). Both pass that binary's own
`pmtiles verify` with exit 0.

## What each one is here for

`dupes-z0z3.pmtiles` has **85 addressed tiles in 67 entries**, so entries carry
run lengths above 1 and several non-adjacent entries share one payload offset. A
reader that ignores run lengths returns 67 tiles and looks fine; `extract`
against this file has to write 85 files.

`leaves-z0z7.pmtiles` has **6 root entries, 6 leaf directories and 21844 tile
entries** covering 21845 addressed tiles, from 2 distinct payloads. It is the
only fixture anywhere that exercises the leaf offset base: a tile entry inside a
leaf is positioned relative to `tile_data_offset`, not to the leaf's own start
and not to `leaf_directories_offset`. None of the three fixtures in the upstream
specification repository has leaf directories at all, so a writer and a reader
that make the same wrong choice here round-trip perfectly.

`raster-z0z2.pmtiles` from the same set is deliberately **not** copied: it
exercises nothing these two do not, and cross-backend equivalence against it
belongs to `libviprs-tests` rather than here.
