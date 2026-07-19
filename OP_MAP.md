# OP_MAP — libviprs op → vips CLI master map (Wave 0 gate D)

> **SOURCE OF TRUTH** for `viprs` command names, CLI shapes, and oracle classes
> (CLI_CONTRACT.md §0). Later waves implement exactly what this table says; nothing
> in Waves 1+ may contradict it.
>
> **Provenance**: every `vips_nickname` below was verified against the author-Mac
> oracle `/opt/homebrew/bin/vips` **8.18.4** (`vips -l` + `vips <op> --help`,
> 2026-07-19). libviprs public surface read at core commit `4629599`
> (all `pub fn` in `src/{arithmetic,composite,resample,resize,colour,draw,bands,
> conversion,convolution,morphology,histogram,mosaicing,freqfilt,create,extract,
> matrix}.rs`, 463 fns, plus the two contract-mandated ops in `raster_ops.rs`).

## Column legend

- **libviprs_fn** — op BASE name after `try_*`/panicking dedup. Builders, accessors,
  setters, `from_name`, Kernel accessors, `DrawOp` constructors and `SdfParams`
  accessors are **not ops** and get no rows (audited per family at the end of each
  section).
- **vips_nickname** — exact vips 8.18.4 spelling (`dE76`, `labelregions`,
  `globalbalance`, …). Rows marked *(fold)* do not become their own subcommand:
  they are variants reachable through flags of the named command (e.g.
  `linear_uchar` → `linear --uchar`). One CLI command per distinct nickname.
- **cli_shape** — CLI_CONTRACT.md §3:
  `S1` image→image · `S2` N-image→image (variadic inputs before OUT) ·
  `S3` image→stdout-scalar(s), no OUT · `S4` image→two-outputs ·
  `S5` creator, OUT first · `S6` draw (`<op> IN OUT --ink …`, documented deviation).
- **oracle_class** — CLI_CONTRACT.md §5: `EXACT` | `EXACT-AFTER-CAST` (EAC) |
  `BOUNDED-TOL` (BT, tol stated in notes) | `FOURIER` | `GOLDEN-ONLY` | `EXCLUDED`
  | `DEFERRED`.

## Count summary

Rows (op base names): **247** — 205 with a real vips differential oracle,
16 GOLDEN-ONLY, 26 EXCLUDED, **0 DEFERRED** (see notes below the table).
Distinct `viprs` subcommands implied (fold-rows collapsed into their parent
command): **164**, every name verified vips-callable (incl. the `crop` alias).
Of those, **151** are differential-backed and **13** are golden-only pins
(7 `draw_*`, `globalbalance`, `gaussnoise`, `perlin`, `worley`, `fractsurf`,
`text`) — squarely inside the contract's "~150–190 with a real vips oracle".

| oracle_class | rows |
|---|---|
| EXACT | 105 |
| EXACT-AFTER-CAST | 29 |
| BOUNDED-TOL | 59 |
| FOURIER | 12 |
| GOLDEN-ONLY | 16 |
| EXCLUDED | 26 |
| DEFERRED | 0 |

| family | rows | EXACT | EAC | BT | FOURIER | GOLDEN | EXCLUDED |
|---|---|---|---|---|---|---|---|
| arithmetic | 94 | 39 | 27 | 7 | 6 | 0 | 15 |
| composite | 2 | 0 | 0 | 2 | 0 | 0 | 0 |
| resample | 15 | 0 | 0 | 13 | 0 | 0 | 2 |
| resize (helpers) | 2 | 0 | 0 | 0 | 0 | 0 | 2 |
| colour | 9 | 0 | 0 | 7 | 0 | 0 | 2 |
| draw | 9 | 0 | 0 | 0 | 0 | 7 | 2 |
| bands | 12 | 12 | 0 | 0 | 0 | 0 | 0 |
| conversion | 21 | 18 | 1 | 2 | 0 | 0 | 0 |
| convolution | 9 | 4 | 0 | 5 | 0 | 0 | 0 |
| morphology | 5 | 5 | 0 | 0 | 0 | 0 | 0 |
| histogram | 15 | 10 | 0 | 4 | 0 | 0 | 1 |
| mosaicing | 3 | 2 | 0 | 0 | 0 | 1 | 0 |
| freqfilt | 6 | 0 | 0 | 0 | 6 | 0 | 0 |
| create | 29 | 3 | 0 | 16 | 0 | 8 | 2 |
| extract | 11 | 11 | 0 | 0 | 0 | 0 | 0 |
| matrix | 3 | 0 | 0 | 3 | 0 | 0 | 0 |
| core (raster_ops) | 2 | 1 | 1 | 0 | 0 | 0 | 0 |
| **total** | **247** | **105** | **29** | **59** | **12** | **16** | **26** |

### Why DEFERRED = 0

The contract reserves DEFERRED for ops that need signed/complex `PixelFormat`
carriers (#283/#285). No public libviprs op *requires* them to exist as a CLI
command: complex-domain ops ship as float-pair Fourier bands via `.v` (class
FOURIER, per §5), and no signed-only op exists in the public surface. The
#283/#285 gaps surface instead as **per-row subset notes** (`cast` cannot target
`char/short/int/complex/double/dpcomplex`; `sign`/`subtract`-style negative
results exist only post-promotion inside f64 math and are compared after the §2
save-cast). Wave agents must keep those subsets red-flagged in `--help` text.

### Cross-cutting missing-scope hooks (§9)

- **matrix-file loader** (vips `.mat` text format → `Kernel` / `&[&[f64]]` /
  `&[&[u8]]`): needed by `conv`, `convsep`, `compass`, `morph` (erode/dilate),
  `recomb`, `maplut` (LUT may be a matrix), `buildlut`, `matrixinvert`,
  `invertlut`. First family to land owns it (§3). `create::from_matrix` is the
  core-side constructor it feeds.
- **`.v` carrier**: mandatory output for FOURIER rows, float creators
  (`mask_*`, `grey` float path, `xyz`, `zone`, `sines`, `eye`, `buildlut`,
  `tonelut`, `sdf`), float-promoting EAC ops when `--no-cast` inspection is
  wanted, and `stats`/`measure` matrix outputs.
- **N-band (>4) outputs**: `Multi8/Multi16` rasters (bandjoin ≥5 bands,
  `hist_find_ndim`) need `.v` or the bands-family `encode_tiff` extension (§2).

---

## arithmetic (src/arithmetic.rs — 131 pub fns → 94 base ops)

### Statistics / scalar outputs

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `avg` | `avg` | S3 | EXACT | prints vips numeric format (`100.000000`); harness float-parse. Integer sums exact in f64. |
| `deviate` | `deviate` | S3 | BOUNDED-TOL | sd formula association; rel eps 1e-9 on the printed scalar. |
| `min` | `min` | S3 | EXACT | optional printed outputs `--x --y` (vips output args); `--size/--out-array` not in core — do not expose. |
| `max` | `max` | S3 | EXACT | as `min`. |
| `minpos` | `min` *(fold)* | S3 | EXACT | no own vips nickname; it IS `min --x --y`. One `viprs min` command serves both. |
| `maxpos` | `max` *(fold)* | S3 | EXACT | fold into `max --x --y`. |
| `stats` | `stats` | S1 | BOUNDED-TOL | out = double matrix image → `.v`; f64 accumulation order, eps 1e-9. |
| `measure` | `measure` | S1 | BOUNDED-TOL | `measure in out h v`; double matrix out via `.v`; eps 1e-9. |
| `find_trim` | `find_trim` | S3 | EXACT | prints 4 ints (left top width height). Core has `--background` only — vips `--threshold` (default 10) / `--line-art` not exposed; pin vips run to defaults. |
| `profile` | `profile` | S4 | EXACT | `profile IN COLS_OUT ROWS_OUT` (vips positional order). |
| `project` | `project` | S4 | EXACT | `project IN COLS_OUT ROWS_OUT`; uint sums. |

### Const / linear

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `linear` | `linear` | S1 | EXACT-AFTER-CAST | `linear IN OUT "a…" "b…"` — vector = one space-separated string (§1). Core fn is scalar `a b`; vector form composes `mul_vec`+`add_vec` in float with **no intermediate cast** (semantically one vips linear). `--uchar` supported. |
| `linear_uchar` | `linear` *(fold)* | S1 | EXACT | `linear --uchar`; uchar out directly, tol 0. |
| `rem_const` | `remainder_const` | S1 | EXACT | `remainder_const in out c` — c is a vector arg. Format-preserving int op. |
| `pow_const` | `math2_const` | S1 | EXACT-AFTER-CAST | `math2_const in out pow "c"`. Transcendental: knife-edge cast pixels possible (libm vs Rust ULP) — if red, demote to BT ≤1 LSB and log it. |
| `add_const` | — | — | EXCLUDED | vips has no `add_const` CLI op; it is `linear 1 c`. No subcommand. |
| `sub_const` | — | — | EXCLUDED | `linear 1 -c`. |
| `mul_const` | — | — | EXCLUDED | `linear c 0`. |
| `div_const` | — | — | EXCLUDED | `linear (1/c) 0`. |
| `floordiv_const` | — | — | EXCLUDED | no vips CLI op (`floor ∘ linear`). |
| `add_vec` | — | — | EXCLUDED | vector form of `linear` (b-vector); building block of the `linear` command, not its own op. |
| `sub_vec` | — | — | EXCLUDED | as `add_vec`. |
| `mul_vec` | — | — | EXCLUDED | vector form of `linear` (a-vector). |
| `div_vec` | — | — | EXCLUDED | as `mul_vec`. |

### Unary / rounding

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `abs` | `abs` | S1 | EXACT | near-no-op on unsigned carriers; meaningful for float `.v` fixtures. |
| `sign` | `sign` | S1 | EXACT-AFTER-CAST | vips emits signed values (−1/0/1); unsigned inputs yield 0/1 which survive the §2 save-cast. Full negative parity needs #283 — note in `--help`. |
| `clamp` | `clamp` | S1 | EXACT | `--min --max` (vips optional args). |
| `floor` | `round` *(fold)* | S1 | EXACT | `round in out floor` — one `viprs round` command, enum `rint|ceil|floor`. |
| `ceil` | `round` *(fold)* | S1 | EXACT | `round … ceil`. |
| `rint` | `round` *(fold)* | S1 | EXACT | `round … rint`. |
| `pos` | — | — | EXCLUDED | identity; vips `copy` covers. |
| `neg` | — | — | EXCLUDED | `linear -1 0`; no vips nickname. |

### Binary / N-ary image ops

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `sub` | `subtract` | S2 | EXACT-AFTER-CAST | #282; promotes (uchar−uchar → signed in vips) — compare only after §2 round-half-even save-cast (negatives clip to 0, unit-pinned). |
| `mul` | `multiply` | S2 | EXACT-AFTER-CAST | vips promotes int formats. |
| `div` | `divide` | S2 | EXACT-AFTER-CAST | float out. |
| `minpair` | `minpair` | S2 | EXACT | format-preserving (vips ≥8.15 nickname verified present in 8.18.4). |
| `maxpair` | `maxpair` | S2 | EXACT | as `minpair`. |
| `sum` | `sum` | S2 | EXACT-AFTER-CAST | vips `in` is an image ARRAY → `viprs sum A B C… OUT` variadic. |
| `max_diff` | — | — | EXCLUDED | libviprs test helper; no vips op (`abs∘subtract→max` composition). |
| `avg_diff` | — | — | EXCLUDED | as `max_diff`. |

### Relational (one `relational` + one `relational_const` command; enum `equal|noteq|less|lesseq|more|moreeq`)

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `more_than` | `relational` *(fold)* | S2 | EXACT | `relational left right out more`; uchar 0/255 out. |
| `more_eq` | `relational` *(fold)* | S2 | EXACT | `moreeq`. |
| `less_than` | `relational` *(fold)* | S2 | EXACT | `less`. |
| `less_eq` | `relational` *(fold)* | S2 | EXACT | `lesseq`. |
| `equal` | `relational` *(fold)* | S2 | EXACT | `equal`. |
| `noteq` | `relational` *(fold)* | S2 | EXACT | `noteq`. |
| `more_than_const` | `relational_const` *(fold)* | S1 | EXACT | `relational_const in out more "c…"`; c is a vector. |
| `more_eq_const` | `relational_const` *(fold)* | S1 | EXACT | `moreeq`. |
| `less_than_const` | `relational_const` *(fold)* | S1 | EXACT | `less`. |
| `less_eq_const` | `relational_const` *(fold)* | S1 | EXACT | `lesseq`. |
| `equal_const` | `relational_const` *(fold)* | S1 | EXACT | `equal`. |
| `noteq_const` | `relational_const` *(fold)* | S1 | EXACT | `noteq`. |

### Bitwise (one `boolean` + one `boolean_const` command; enum `and|or|eor|lshift|rshift`)

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `bitand` | `boolean` *(fold)* | S2 | EXACT | `boolean left right out and`. |
| `bitor` | `boolean` *(fold)* | S2 | EXACT | `or`. |
| `bitxor` | `boolean` *(fold)* | S2 | EXACT | `eor` (vips spelling!). |
| `bitand_const` | `boolean_const` *(fold)* | S1 | EXACT | `boolean_const in out and "c…"`; c vector (core takes i64). |
| `bitor_const` | `boolean_const` *(fold)* | S1 | EXACT | `or`. |
| `bitxor_const` | `boolean_const` *(fold)* | S1 | EXACT | `eor`. |
| `lshift` | `boolean_const` *(fold)* | S1 | EXACT | `lshift`; core is const-shift only (no 2-image shift). |
| `rshift` | `boolean_const` *(fold)* | S1 | EXACT | `rshift`. |
| `band_and` | — | — | EXCLUDED | alias of `bitand` kept for ported-test naming; not a distinct op (bands-across is `bandbool`). |
| `bitnot` | — | — | EXCLUDED | vips `boolean` has no NOT (vips `invert` is photographic, which libviprs lacks). Library extension only. |

### Windowed / matrix / alpha

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `scaleimage` | `scale` | S1 | BOUNDED-TOL | vips nickname is `scale` (NOT scaleimage). `--log --exp`; ≤1 LSB uchar (log path transcendental). |
| `stdif` | `stdif` | S1 | BOUNDED-TOL | core exposes `width height` only (vips `--a --m0 --b --s0` defaults assumed); ≤1 LSB. |
| `recomb` | `recomb` | S1 | EXACT-AFTER-CAST | `recomb in out m` — **matrix FILE arg** (shared loader). |
| `premultiply` | `premultiply` | S1 | BOUNDED-TOL | #406-418; float out in vips; ≤1 LSB post-cast (or f32 eps 1e-5 via `.v`). `--max-alpha` not in core. |
| `unpremultiply` | `unpremultiply` | S1 | BOUNDED-TOL | division; same bounds as `premultiply`. |

### Trig / log / exp (one `math` command; enum verified: `sin cos tan asin acos atan log log10 exp exp10 sinh cosh tanh asinh acosh atanh`)

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `sin` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | float-promoting; §2 save-cast then tol 0. ULP knife-edge caveat as `pow_const`. |
| `cos` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | — |
| `tan` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | — |
| `asin` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | — |
| `acos` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | — |
| `atan` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | — |
| `sinh` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | — |
| `cosh` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | — |
| `tanh` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | — |
| `asinh` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | — |
| `acosh` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | — |
| `atanh` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | — |
| `log` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | — |
| `log10` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | — |
| `exp` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | — |
| `exp10` | `math` *(fold)* | S1 | EXACT-AFTER-CAST | — |
| `atan2` | `math2` *(fold)* | S2 | EXACT-AFTER-CAST | `math2 left right out atan2`. |
| `pow` | `math2` *(fold)* | S2 | EXACT-AFTER-CAST | `pow`. |
| `wop` | `math2` *(fold)* | S2 | EXACT-AFTER-CAST | `wop`. |

### Complex (float-pair bands, `Interpretation::Fourier`, `.v` carrier)

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `complexform` | `complexform` | S2 | FOURIER | two real inputs → complex `.v` out; f64 band-pair compare, eps 1e-6. |
| `polar` | `complex` *(fold)* | S1 | FOURIER | `complex in out polar`; `.v` in/out. |
| `rect` | `complex` *(fold)* | S1 | FOURIER | `rect`. |
| `conj` | `complex` *(fold)* | S1 | FOURIER | `conj`. |
| `real` | `complexget` *(fold)* | S1 | FOURIER | `complexget in out real`; complex `.v` in → real out. |
| `imag` | `complexget` *(fold)* | S1 | FOURIER | `imag`. |

### Hough

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `hough_line` | `hough_line` | S1 | EXACT | uint accumulator; core has no params — vips `--width --height` defaults (256×256) must be pinned at fixture-gen time. |
| `hough_circle` | `hough_circle` | S1 | EXACT | core takes `min_radius max_radius` only; vips `--scale` defaults to 3 — pin explicit `--scale`/radii flags when generating references so both sides compute the same parameter space. |

Non-op public API in this file (no rows): none — all 131 fns dedup to the 94 bases above.

---

## composite (src/composite.rs — 3 pub fns → 2 base ops)

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `composite` | `composite` | S2 | BOUNDED-TOL | vips takes image ARRAY + mode ARRAY; core supports exactly 2 images / 1 mode → `viprs composite BASE OVERLAY OUT "mode"` (subset; reject >2 inputs with exit 2). ≤1 LSB (premultiplied f32 blend). 25 `CompositeMode`s = vips BlendMode names. |
| `composite2` | `composite2` | S2 | BOUNDED-TOL | `composite2 base overlay out mode`; vips `--x --y --compositing-space --premultiplied` not in core (defaults). ≤1 LSB. |

---

## resample (src/resample.rs — 43 pub fns → 15 base ops) + resize helpers

All BOUNDED-TOL per the premultiply/rounding campaign #406-418; state ≤1 LSB
(8-bit) / ≤1 LSB per channel (16-bit) unless noted.

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `shrink` | `shrink` | S1 | BOUNDED-TOL | `shrink in out hshrink vshrink` (f64 args); vips `--ceil` not in core. |
| `shrinkh` | `shrinkh` | S1 | BOUNDED-TOL | integer factor in core (u32) vs vips gint — match. |
| `shrinkv` | `shrinkv` | S1 | BOUNDED-TOL | — |
| `reduce` | `reduce` | S1 | BOUNDED-TOL | `--kernel` (core `ReduceKernel`; `from_name` maps CLI string) `--gap` not in core. |
| `reduceh` | `reduceh` | S1 | BOUNDED-TOL | — |
| `reducev` | `reducev` | S1 | BOUNDED-TOL | — |
| `resize` | `resize` | S1 | BOUNDED-TOL | `resize in out scale`; `try_resize_with` options → `--kernel --vscale`(core `ResizeOptions`); vips `--gap` if core grows it. Dedup of `resize`/`resize_with`/`try_resize_with`. |
| `affine` | `affine` | S1 | BOUNDED-TOL | `affine in out "a b c d"` (matrix = one space-separated string); `--interpolate`; `try_affine_with` extras (oarea/offsets) as flags if exposed. |
| `similarity` | `similarity` | S1 | BOUNDED-TOL | `--scale --angle` (vips optional args); `_with` variant folds. |
| `rotate` | `rotate` | S1 | BOUNDED-TOL | `rotate in out angle`; `_with` folds. |
| `mapim` | `mapim` | S2 | BOUNDED-TOL | `mapim in out index` — index image is a 2nd INPUT; `--interpolate`. Index is float `.v` for exactness. |
| `thumbnail` | `thumbnail` | S1 | BOUNDED-TOL | vips arg is a **source filename**, not a decoded image (`thumbnail filename out width`); `--height --crop --linear --export-profile` per core options; dedups `thumbnail`/`try_thumbnail`/`thumbnail_with_options`/`thumbnail_with_profile`/free `thumbnail`/`thumbnail_crop`. PNG inputs only (no jpeg shrink-on-load divergence). |
| `thumbnail_image` | `thumbnail_image` | S1 | BOUNDED-TOL | decoded-image variant. |
| `thumbnail_buffer` | — | — | EXCLUDED | buffer-input variant has no CLI file surface; stdin `-` streaming is §9 deferred scope. `viprs thumbnail` covers files. |
| `constant_u8` | — | — | EXCLUDED | test helper; `black` + `linear` cover. |
| `downscale_half` *(resize.rs)* | — | — | EXCLUDED | internal helper; `shrink 2 2` covers. |
| `downscale_to` *(resize.rs)* | — | — | EXCLUDED | internal helper; `resize`/`thumbnail` cover. |

Non-op public API (no rows): `ReduceKernel::from_name`, `Interpolate::from_name`
(CLI string→enum plumbing used by `--kernel`/`--interpolate`).

---

## colour (src/colour.rs — 19 pub fns → 9 base ops)

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `colourspace` | `colourspace` | S1 | BOUNDED-TOL | `colourspace in out space` + `--source-space`; contract "colour round-trips": ≤1 LSB uchar, 1e-4 float via `.v`. |
| `de76` | `dE76` | S2 | BOUNDED-TOL | capital-E spelling verified. Float ΔE out via `.v`, eps 1e-4. |
| `de00` | `dE00` | S2 | BOUNDED-TOL | mirrors libvips's dE00. eps 1e-4. |
| `de00_sharma` | — | — | EXCLUDED | Sharma-2005 variant of CIEDE2000; no distinct vips nickname (`dE00` maps to `de00`). Library extension; revisit only with an `extension` flag row. |
| `de_cmc` | `dECMC` | S2 | BOUNDED-TOL | eps 1e-4. |
| `icc_import` | `icc_import` | S1 | BOUNDED-TOL | `--input-profile --intent --pcs` (core `_with` folds). **Caveat**: libviprs ships a native ICC engine; homebrew vips uses lcms2. Restrict differential fixtures to matrix-shaper RGB profiles (sRGB); CMYK combos diverge by design (core targets the no-lcms approximation) → mark those cases GOLDEN-ONLY in the test files, tol from measurement (start 1e-3 float / ≤2 LSB uchar). |
| `icc_export` | `icc_export` | S1 | BOUNDED-TOL | `--output-profile --intent`; same lcms caveat. |
| `icc_transform` | `icc_transform` | S1 | BOUNDED-TOL | `icc_transform in out output-profile` (profile is positional in vips); same caveat. |
| `constant` | — | — | EXCLUDED | creator helper with interpretation tag; `black`+`linear`+`copy --interpretation` cover. |

---

## draw (src/draw.rs — 28 pub fns → 9 base ops)

All GOLDEN-ONLY (§5): vips `draw_*` are in-place mutators whose CLI discards the
result, so there is NO vips CLI oracle. Reference images are committed ONCE from
a libviprs-generated fixture; tests are regression pins and must say so.
Shape S6 documented deviation: `viprs draw_* IN OUT --ink "r g b" <args…>`
(ink parsing per-format incl. 16-bit byte order is §9 missing-scope).

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `draw_circle` | `draw_circle` | S6 | GOLDEN-ONLY | `cx cy radius`; `--fill` folds `draw_circle_filled`. |
| `draw_rect` | `draw_rect` | S6 | GOLDEN-ONLY | `left top width height`; `--fill` folds `draw_rect_filled`. |
| `draw_line` | `draw_line` | S6 | GOLDEN-ONLY | `x1 y1 x2 y2`. |
| `draw_flood` | `draw_flood` | S6 | GOLDEN-ONLY | `x y`; `--equal` folds `draw_flood_blob` (vips "flood while equal to edge"). |
| `draw_mask` | `draw_mask` | S6 | GOLDEN-ONLY | mask is an image FILE arg before x y (vips order `image ink mask x y`). |
| `draw_smudge` | `draw_smudge` | S6 | GOLDEN-ONLY | no ink: `left top width height`. |
| `draw_image` | `draw_image` | S6 | GOLDEN-ONLY | `sub x y`; vips `--mode` (set/add) not in core (plain paste). |
| `draw` | — | — | EXCLUDED | generic `DrawOp` dispatcher; API plumbing, not a command. |
| `put_pixel` | — | — | EXCLUDED | 1×1 `draw_rect`; no vips nickname. |

Non-op public API (no rows): `DrawOp` constructors `Circle::outline/filled`,
`Rect::outline/filled`, `Line::new`, `Flood::bounded/blob`, `Mask::new`,
`Smudge::new`, `Image::new` (per instructions: constructors excluded).

---

## bands (src/bands.rs — 24 pub fns → 12 base ops)

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `bandjoin` | `bandjoin` | S2 | EXACT | vips `in` = image ARRAY → `viprs bandjoin A B [C…] OUT`; core joins pairwise (loop for >2 — pure concat, associativity exact). ≥5-band outputs need `.v`/N-band TIFF. |
| `bandjoin_const` | `bandjoin_const` *(fold)* | S1 | EXACT | single-value case of the same command (`c` vector length 1). |
| `bandjoin_vec` | `bandjoin_const` | S1 | EXACT | `bandjoin_const in out "c…"` — the general vector form is THE command; `bandjoin_const` fn is its 1-element case. |
| `bandfold` | `bandfold` | S1 | EXACT | `--factor`. |
| `bandunfold` | `bandunfold` | S1 | EXACT | `--factor`. |
| `bandmean` | `bandmean` | S1 | BOUNDED-TOL | ≤1 LSB: core FLOORS the per-pixel integer mean (truncating division) vs vips ROUND-to-nearest; a non-divisible band sum diverges by at most one LSB (core-vs-vips rounding, not a CLI bug). |
| `bandrank` | `bandrank` | S2 | EXACT | variadic inputs + `--index` (default median). |
| `bandand` | `bandbool` *(fold)* | S1 | EXACT | `bandbool in out and` — one command, enum `and|or|eor`. |
| `bandor` | `bandbool` *(fold)* | S1 | EXACT | `or`. |
| `bandeor` | `bandbool` *(fold)* | S1 | EXACT | `eor`. |
| `extract_band` | `extract_band` | S1 | EXACT | `extract_band in out band` + `--n` (count). |
| `extract_bands` | `extract_band` *(fold)* | S1 | EXACT | IS `extract_band --n N`. |

---

## conversion (src/conversion.rs — 53 pub fns → 21 base ops)

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `copy` | `copy` | S1 | EXACT | vips `copy` header-tweak flags: `--interpretation --xres --yres --xoffset --yoffset` (vips 8.18.4 `copy` exposes only width/height/bands/format/coding/interpretation/xres/yres/xoffset/yoffset). `--orientation` is a DELIBERATE viprs-only extension (NOT a vips `copy` flag), kept **hidden** (`.hide(true)`) off the parity surface solely to mint the `autorot` oriented `.v` fixture; recorded here + in CLI_CONTRACT §3-deviations, NOT counted as vips parity (adversarial-review conversion finding 4). |
| `cast` | `cast` | S1 | EXACT | `cast in out format`; core formats: `uchar ushort float` (+gray/multi widths) ONLY — `char short int uint complex double dpcomplex` targets rejected exit 2 with a #283/#285 pointer; vips `--shift` not in core. float→int path uses §2 round-half-even. |
| `fliphor` | `flip` *(fold)* | S1 | EXACT | `flip in out horizontal` — one command, enum `horizontal|vertical`. |
| `flipver` | `flip` *(fold)* | S1 | EXACT | `vertical`. |
| `rot` | `rot` | S1 | EXACT | `rot in out d90|d180|d270` (core `Angle`). |
| `rot45` | `rot45` | S1 | EXACT | `--angle d45…d315` (core `Angle45`); odd-square inputs only. |
| `byteswap` | `byteswap` | S1 | EXACT | meaningful for 16-bit; compare via `.v` (PNG re-encode normalises byte order). |
| `msb` | `msb` | S1 | EXACT | `--band`. |
| `grid` | `grid` | S1 | EXACT | `grid in out tile-height across down` (vips positional order verified). |
| `flatten` | `flatten` | S1 | BOUNDED-TOL | `--background` vector; vips `--max-alpha` not in core. Alpha blend rounding ≤1 LSB. |
| `ifthenelse` | `ifthenelse` | S2 | EXACT | `ifthenelse cond in1 in2 out` (3 inputs); vips `--blend` not in core (hard select). |
| `autorot` | `autorot` | S1 | EXACT | orientation lives in TIFF/`.v` metadata — PNG fixtures are no-ops; use oriented-TIFF fixture. |
| `wrap` | `wrap` | S1 | EXACT | core has no args (vips `--x --y` default w/2,h/2 match). |
| `gamma` | `gamma` | S1 | EXACT-AFTER-CAST | `--exponent` (vips default 2.4); contract §5 example. |
| `falsecolour` | `falsecolour` | S1 | EXACT | fixed LUT map (UK spelling verified). |
| `addalpha` | `addalpha` | S1 | EXACT | appends opaque alpha. |
| `arrayjoin` | `arrayjoin` | S2 | EXACT | variadic + `--across --shim`; vips extra layout args (halign/valign/hspacing/background) not in core. |
| `grey` | `grey` | S5 | BOUNDED-TOL | `grey out width height [--uchar]`; float ramp path via `.v` eps 1e-6, uchar path ≤1 LSB (expected 0). |
| `identity` | `identity` | S5 | EXACT | `identity out [--ushort]`; vips `--bands --size` not in core. |
| `identity_ushort` | `identity` *(fold)* | S5 | EXACT | IS `identity --ushort`. |
| `switch` | `switch` | S2 | EXACT | `switch A B C… OUT` (vips `tests` image array). |

Non-op public API (no rows): `RasterCopyBuilder` (`for_format`, setters
`interpretation/xres/yres/xoffset/yoffset/orientation`, `build`) and Raster
accessors (`bands`, `interpretation`, `xres`, `yres`, `xoffset`, `yoffset`,
`orientation`) — flag/introspection plumbing for `copy`/`info`.

---

## convolution (src/convolution.rs — 22 pub fns → 9 base ops)

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `gaussmat` | `gaussmat` | S5 | BOUNDED-TOL | `gaussmat out sigma min-ampl --separable --precision`; matrix out → `.mat`/`.v`. f64 eps 1e-9; integer-precision path expected tol 0. |
| `logmat` | `logmat` | S5 | BOUNDED-TOL | as `gaussmat`; `logmat_with_precision` folds into `--precision`. |
| `conv` | `conv` | S1 | EXACT | `conv in out mask.mat` — **matrix FILE arg** (shared loader → `Kernel`); `--precision` (core `Precision`); integer precision tol 0, float-precision combos noted BT ≤1 LSB in the case files. |
| `convsep` | `convsep` | S1 | EXACT | separable mask file; same precision note. |
| `compass` | `compass` | S1 | BOUNDED-TOL | mask file + `--times --angle --combine --precision` (core signature); default float precision → ≤1 LSB. |
| `gaussblur` | `gaussblur` | S1 | EXACT | `gaussblur in out sigma --min-ampl --precision`; vips default precision=int → tol 0; float-precision combos BT. |
| `sharpen` | `sharpen` | S1 | BOUNDED-TOL | core `--sigma --m1 --m2` (vips also x1/y2/y3/mode — defaults); LabS float path ≤1 LSB. |
| `spcor` | `spcor` | S2 | BOUNDED-TOL | `spcor in ref out`; float correlation out via `.v`, eps 1e-5. |
| `fastcor` | `fastcor` | S2 | EXACT | int sum-of-squared-differences (uint out). |

Non-op public API (no rows): `Kernel` accessors `width/height/max`.

---

## morphology (src/morphology.rs — 10 pub fns → 5 base ops)

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `erode` | `morph` *(fold)* | S1 | EXACT | `morph in out mask.mat erode` — one command, enum verified `erode|dilate`; mask FILE → `&[&[u8]]` (0/128/255 = vips 0/128/255 dont-care semantics). |
| `dilate` | `morph` *(fold)* | S1 | EXACT | `dilate`. |
| `rank` | `rank` | S1 | EXACT | `rank in out width height index`. |
| `countlines` | `countlines` | S3 | EXACT | `countlines in horizontal|vertical` prints `nolines` double; rational value, float-parse. |
| `label_regions` | `labelregions` | S4 | EXACT | ONE WORD nickname verified. `labelregions IN MASK_OUT` + printed `--segments` output int. Region numbering must match vips scan order (pinned by ported tests). |

*(This is the Wave-1 reference family: S1 + mask-file arg + S3 scalar + S4 two-output in 5 commands.)*

---

## histogram (src/histogram.rs — 29 pub fns → 15 base ops)

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `hist_find` | `hist_find` | S1 | EXACT | ushort/uint counts; out via `.v`/`.mat` (1×256 image). |
| `hist_find_band` | `hist_find` *(fold)* | S1 | EXACT | IS `hist_find --band N`. |
| `hist_find_indexed` | `hist_find_indexed` | S2 | EXACT | `hist_find_indexed in index out` (2 inputs). |
| `hist_find_ndim` | `hist_find_ndim` | S1 | EXACT | `--bins`; N-dim output may exceed 4 bands → `.v`. |
| `hist_cum` | `hist_cum` | S1 | EXACT | integer cumulative. |
| `hist_norm` | `hist_norm` | S1 | EXACT | integer renormalisation. |
| `hist_match` | `hist_match` | S2 | BOUNDED-TOL | `hist_match in ref out`; float LUT build then int map — ≤1 LSB. |
| `hist_plot` | `hist_plot` | S1 | EXACT | deterministic plot raster. |
| `hist_entropy` | `hist_entropy` | S3 | BOUNDED-TOL | float scalar (log2); printed-value eps 1e-9. |
| `hist_ismonotonic` | `hist_ismonotonic` | S3 | EXACT | boolean output arg (`monotonic`); harness parses vips's printed bool/int form. |
| `hist_equal` | `hist_equal` | S1 | BOUNDED-TOL | vips `--band` not in core (all-bands only). Equalisation LUT rounding ≤1 LSB (expected 0). |
| `hist_local` | `hist_local` | S1 | BOUNDED-TOL | `width height --max-slope` (CLAHE); ≤1 LSB. |
| `maplut` | `maplut` | S2 | EXACT | `maplut in out lut` — LUT is 2nd input (image or matrix file); `--band` (vips) not in core. |
| `case` | — | — | EXCLUDED | core takes CONST cases (`&[f64]`); vips `case` requires an image array (`case index cases… out`) — no CLI mirror without N-image case support. Revisit if core grows image-cases. |
| `percent` | `percent` | S3 | EXACT | `percent in percent` prints int threshold. |

---

## mosaicing (src/mosaicing.rs — 6 pub fns → 3 base ops)

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `merge` | `merge` | S2 | EXACT | `merge ref sec out direction dx dy` + `--mblend` (core default 10 = vips). Integer blend ramp on int inputs. |
| `mosaic` | `mosaic` | S2 | EXACT | `mosaic ref sec out direction xref yref xsec ysec`; core pins vips defaults bandno 0, hwindow 5, harea 15, mblend 10 (per rustdoc). Discrete tie-point search must agree exactly or outputs diverge wholesale — the differential doubles as the search pin. |
| `global_balance` | `globalbalance` | S1 | GOLDEN-ONLY | ONE WORD nickname. Core requires the libviprs join-tree metadata blob (`JOIN_TREE_FIELD`) that only viprs `merge`/`mosaic` outputs carry; vips's globalbalance reads its own image-history records instead — the two metadata channels are mutually unreadable, so NO cross-oracle exists. Regression pin: viprs mosaic→global_balance pipeline fixture. Exit 1 on inputs without join-tree metadata. |

---

## freqfilt (src/freqfilt.rs — 12 pub fns → 6 base ops)

Complex = float-pair bands stamped `Interpretation::Fourier`; all IO via `.v`;
compare as f64 band-pairs at a per-op MEASURED absolute epsilon (see
`cli_freqfilt_diff.rs` / PROVENANCE.md), each sized above the op's
f32-quantisation floor — NOT a fixed 1e-6 relative (contract §5 FOURIER).

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `fwfft` | `fwfft` | S1 | FOURIER | real/complex in → complex `.v` out. |
| `invfft` | `invfft` | S1 | FOURIER | complex in; `--real` folds `invfft_real`. |
| `invfft_real` | `invfft` *(fold)* | S1 | FOURIER | IS `invfft --real` (verified vips option). |
| `freqmult` | `freqmult` | S2 | FOURIER | `freqmult in mask out` — mask is 2nd input (real float mask from `mask_*` creators). |
| `spectrum` | `spectrum` | S1 | FOURIER | displayable log-magnitude; still FFT-derived float → `.v` compare. |
| `phasecor` | `phasecor` | S2 | FOURIER | `phasecor in in2 out`. |

---

## create (src/create.rs — 56 pub fns → 29 base ops)

Creators are S5 (OUT first, §3). Float-valued creators write `.v` for the
differential; uchar variants can use PNG.

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `black` | `black` | S5 | EXACT | `black out width height --bands`. |
| `black_bands` | `black` *(fold)* | S5 | EXACT | IS `black --bands N`. |
| `new_from_image` | — | — | EXCLUDED | binding-style helper (vips_image_new_from_image); no CLI nickname. `black`+`linear` cover. |
| `xyz` | `xyz` | S5 | EXACT | integer indices in float carrier → `.v` tol 0; vips `--csize/dsize/esize` not in core (2-D only). |
| `eye` | `eye` | S5 | BOUNDED-TOL | `--uchar`; vips `--factor` not in core. Trig → f32 eps 1e-6 / ≤1 LSB uchar. |
| `zone` | `zone` | S5 | BOUNDED-TOL | zone plate trig; same bounds. Core is float-out only — vips `--uchar` not exposed. |
| `sines` | `sines` | S5 | BOUNDED-TOL | vips `--hfreq --vfreq --uchar` not in core (defaults only). |
| `gaussnoise` | `gaussnoise` | S5 | GOLDEN-ONLY | PRNG differs from vips even seeded (`--sigma --mean --seed`); committed libviprs fixture, regression pin. |
| `gaussnoise_seeded` | `gaussnoise` *(fold)* | S5 | GOLDEN-ONLY | IS `gaussnoise --seed N`; deterministic across viprs runs — the pin uses it. |
| `perlin` | `perlin` | S5 | GOLDEN-ONLY | vips `--cell-size` not in core; PRNG/gradient tables differ. |
| `perlin_seeded` | `perlin` *(fold)* | S5 | GOLDEN-ONLY | `--seed`. |
| `worley` | `worley` | S5 | GOLDEN-ONLY | — |
| `worley_seeded` | `worley` *(fold)* | S5 | GOLDEN-ONLY | `--seed`. |
| `fractsurf` | `fractsurf` | S5 | GOLDEN-ONLY | `fractsurf out width height fractal-dimension`; built on gaussnoise PRNG → no oracle. |
| `buildlut` | `buildlut` | S1 | BOUNDED-TOL | vips `in` is a matrix IMAGE → `viprs buildlut in.mat out` (matrix FILE via shared loader feeding `&[Vec<f64>]`). f64 linear interp, eps 1e-10 (expected 0). |
| `tonelut` | `tonelut` | S5 | BOUNDED-TOL | core is all-defaults (vips `--in-max --out-max --Lb --Lw --Ps --Pm --Ph --S --M --H` not exposed); f64 curve eps 1e-9. |
| `from_matrix` | — | — | EXCLUDED | matrix-Raster constructor — it is the core half of the §9 matrix-file loader (feeds `conv`/`recomb`/`buildlut`/`matrixinvert`), not a command. |
| `mask_ideal` | `mask_ideal` | S5 | BOUNDED-TOL | `out width height frequency-cutoff --nodc`; vips `--uchar --optical --reject` not in core. Hard threshold → expected tol 0, f32 eps guards cutoff knife-edge. |
| `mask_ideal_ring` | `mask_ideal_ring` | S5 | BOUNDED-TOL | `+ ringwidth --nodc`. |
| `mask_ideal_band` | `mask_ideal_band` | S5 | BOUNDED-TOL | `frequency-cutoff-x frequency-cutoff-y radius`; core has NO `--nodc` here (subset). |
| `mask_gaussian` | `mask_gaussian` | S5 | BOUNDED-TOL | `+ amplitude-cutoff --nodc`; exp() → f32 eps 1e-6. |
| `mask_gaussian_ring` | `mask_gaussian_ring` | S5 | BOUNDED-TOL | `+ ringwidth --nodc`. |
| `mask_gaussian_band` | `mask_gaussian_band` | S5 | BOUNDED-TOL | `fcx fcy radius amplitude-cutoff` (no nodc in core). |
| `mask_butterworth` | `mask_butterworth` | S5 | BOUNDED-TOL | `order frequency-cutoff amplitude-cutoff --nodc`. |
| `mask_butterworth_ring` | `mask_butterworth_ring` | S5 | BOUNDED-TOL | `+ ringwidth --nodc`. |
| `mask_butterworth_band` | `mask_butterworth_band` | S5 | BOUNDED-TOL | fullest core signature: `order fcx fcy radius amplitude-cutoff --uchar --optical --nodc` — flag surfaces differ per mask op; CLI must expose exactly what core has, no invented parity. |
| `mask_fractal` | `mask_fractal` | S5 | BOUNDED-TOL | `fractal-dimension`; pow() → f32 eps 1e-6. |
| `sdf` | `sdf` | S5 | BOUNDED-TOL | `sdf out width height shape` + `--a "x y" --b "x y" --r` (`SdfParams`); shapes `circle|box|rounded-box|line`. Core mirrors the C f32 math exactly → expected tol 0, keep f32 eps 1e-6 for hypotf ULP. |
| `text` | `text` | S5 | GOLDEN-ONLY | Pango/fontconfig rendering differs across hosts; committed libviprs fixture. `text out "string" --font --dpi --width` per core surface. |

Non-op public API (no rows): `SdfParams` accessors `max_value`/`min_value`.

---

## extract (src/extract.rs — 19 pub fns → 11 base ops)

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `extract_area` | `extract_area` | S1 | EXACT | `extract_area input out left top width height`. |
| `crop` | `crop` | S1 | EXACT | vips-callable ALIAS of extract_area (verified: `vips crop` runs; `vips -l` lists it under extract_area). Register as its own clap command dispatching to the same handler. |
| `embed` | `embed` | S1 | EXACT | `embed in out x y width height --extend --background` (extend enum `black copy repeat mirror white background`). |
| `gravity` | `gravity` | S1 | EXACT | `gravity in out direction width height --extend --background` (core `CompassDirection`). |
| `replicate` | `replicate` | S1 | EXACT | `replicate in out across down`. |
| `zoom` | `zoom` | S1 | EXACT | `zoom input out xfac yfac`. |
| `subsample` | `subsample` | S1 | EXACT | `subsample input out xfac yfac`. |
| `insert` | `insert` | S2 | EXACT | `insert main sub out x y --expand --background`. |
| `smartcrop` | `smartcrop` | S1 | EXACT | `smartcrop input out width height --interesting none|centre|entropy|attention|low|high|all`. Discrete window choice: matches exactly or fails loudly — attention/entropy paths are the risk; if a fixture combo drifts, demote THAT case to GOLDEN-ONLY in the test file, not the op. |
| `smartcrop_with_coords` | `smartcrop` *(fold)* | S1 | EXACT | vips optional OUTPUT args `--attention-x --attention-y` (printed) — same command. |
| `smartcrop_with_coords_premultiplied` | `smartcrop` *(fold)* | S1 | EXACT | `--premultiplied` flag (verified vips option). |

---

## matrix (src/matrix.rs — 6 pub fns → 3 base ops)

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `matrixinvert` | `matrixinvert` | S1 | BOUNDED-TOL | matrix FILE in → matrix out (`.mat`); Gaussian-elimination pivot order → f64 eps 1e-9. |
| `invertlut` | `invertlut` | S1 | BOUNDED-TOL | matrix (buildlut-style) in; `--size` (vips default 256); f64 eps 1e-9. |
| `invertlut_size` | `invertlut` *(fold)* | S1 | BOUNDED-TOL | IS `invertlut --size N`. |

---

## core raster ops (src/raster_ops.rs — outside the 16 audited modules, contract-mandated)

`add` and `getpoint` live in `raster_ops.rs`, not in an op family module, but the
frozen contract hard-codes both (`viprs add a.png b.png out.png` §1; `vips
getpoint` .5-fixture pins §2). They are in-scope rows; the arithmetic wave
implements them.

| libviprs_fn | vips_nickname | cli_shape | oracle_class | notes |
|---|---|---|---|---|
| `add` | `add` | S2 | EXACT-AFTER-CAST | **Accepted surface = uchar-only, equal-bands** (the exact set where core == vips). u8+u8→u16 widening compares tol 0. `add` **rejects float AND 16-bit inputs** (exit 1): core keeps a 16-bit input at 16-bit and SATURATES the sum at 65535 (`raster_ops.rs`), whereas vips promotes ushort→uint and returns the true sum — so 16-bit is rejected, NOT silently saturated (wide/float addition lands with a later arithmetic batch; core-side follow-up filed). Also a documented SUBSET of vips on bands: vips `add` BAND-BROADCASTS a 1-band operand across a multi-band one; core requires EQUAL band counts and the CLI keeps the exit-1 rejection (band-broadcast parity = core-side follow-up, not a regression). uchar int fixtures only; float/16-bit combos logged as skipped per §7. |
| `getpoint` | `getpoint` | S3 | EXACT | `getpoint in x y` prints the band vector in vips numeric format. Oracle is the NUMERIC compare (float-parse + eps, §3); stdout TEXT formatting is NOT a pinned parity surface (§9) — non-dyadic float pixels print `f64::to_string` of the widened f32 (e.g. f32 0.1 → `0.10000000149011612`), which the numeric-eps compare carries regardless of vips's text form (pinned by the `getpoint_float_nd` non-dyadic case; the deliberately-dyadic `getpoint_float` fixture only sidesteps text, not the numeric oracle). |

---

## Audit trail — public fns per module vs rows

| module | pub fns | try/panic twins + option-variant folds | non-op API (no rows) | base rows |
|---|---|---|---|---|
| arithmetic.rs | 131 | 37 twins | 0 | 94 |
| composite.rs | 3 | 1 | 0 | 2 |
| resample.rs | 43 | 26 (twins + `_with`/free-fn folds) | 2 (`from_name` ×2) | 15 |
| resize.rs | 2 | 0 | 0 | 2 |
| colour.rs | 19 | 10 (twins + `_with` folds) | 0 | 9 |
| draw.rs | 28 | 9 (try twins + `_filled` merged into base fns) | 10 (DrawOp constructors) | 9 |
| bands.rs | 24 | 12 twins | 0 | 12 |
| conversion.rs | 53 | 17 twins | 15 (builder + accessors) | 21 |
| convolution.rs | 22 | 10 (twins + `logmat_with_precision`) | 3 (Kernel accessors) | 9 |
| morphology.rs | 10 | 5 twins | 0 | 5 |
| histogram.rs | 29 | 14 twins | 0 | 15 |
| mosaicing.rs | 6 | 3 twins | 0 | 3 |
| freqfilt.rs | 12 | 6 twins | 0 | 6 |
| create.rs | 56 | 25 twins | 2 (SdfParams accessors) | 29 |
| extract.rs | 19 | 8 twins | 0 | 11 |
| matrix.rs | 6 | 3 twins | 0 | 3 |
| raster_ops.rs (partial) | 2 | 0 | 0 | 2 |
| **total** | **465** | | | **247** |
