# Fixtures for `viprs vector`

`ladder.pmtiles` is a 2,256 byte vector archive written by `libviprs::vector::convert::convert`
from a synthetic drawing of twelve horizontal lines and one vertical, at `max_zoom` 3, named
`Ladder`. It exists so `viprs vector info` and `viprs vector validate` have an archive to read
in a build that has no DWG provider. It passes `viprs vector validate`.

`sample_AC1018.dwg` is ACadSharp's own `samples/sample_AC1018.dwg` (v3.7.1, MIT, licence in
`LICENSE-ACadSharp.txt`), the same file as libviprs `tests/fixtures/cad/sample_AC1018.dwg`.

- bytes: 1097214
- sha256: `7d26f908516ec5fb89cc8054af33aff233b68a7b63208bbc5b7f9f4d9bff9866`

It is the input for the cells that need the native reader (`--features dwg-acadsharp` and an
archive `acadsharp-rs` can link). They skip, saying so, when the reader is not linked, unless
`LIBVIPRS_REQUIRE_ACADSHARP_NATIVE=1` turns the skip into a failure.

`vc_layers.dwg` is libviprs `tests/fixtures/pvec/provider/vc_layers.dwg`, copied byte for byte (its provenance is that
directory's `PROVENANCE.md`: written by ACadSharp's `DwgWriter` 3.7.1 at AC1032 for libviprs#1235). Its header declares no
unit and its one paper layout, `Layout1`, is empty, so `viprs pvec convert` has a drawing with no sheet to print and
`--model` has one that needs `--unit` (libviprs#1566).

- bytes: 10892
- sha256: `af2f9edf744fd0e94215415ad33cf93a739fe5692b272a523eda9603d14279c5`

`g13_deep_blocks.dwg` is libviprs `tests/fixtures/cad/g13/g13_deep_blocks.dwg` copied byte for byte at the CLI's
core pin (`71cbbe0`); its provenance is that directory's `PROVENANCE.md` (written by ACadSharp's `DwgWriter` 3.7.1, content
libviprs's own, MIT). `g13_deep_blocks.dwg` nests INSERTs six deep, so `--max-block-depth` can be tripped above 0 and both
readers can be held to the same depth.

- bytes: 11051, sha256: `2749f007a82dd4156b61c151ebb6097cb1d36d4551d6e31611b8d39e86fb0f19`
