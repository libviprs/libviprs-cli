# JBIG2 fixture for `viprs pvec convert` (libviprs-cli#151)

`generic_template0.jbig2` is libviprs `tests/fixtures/jbig2/generic_template0.jbig2` copied byte for byte at the CLI's core pin
(`4b443f2f`). That one is itself copied byte for byte from `hayro-jbig2/tests/streams/generic_template0.jbig2` of
https://github.com/LaurenzV/hayro at commit `dee147646768e41f62ff4c327c649f63f57fbee5` (Apache-2.0 OR MIT, the crate's own
licence). Per that suite's README the streams come from a JBIG2 encoder written for it, so no third party image data is in it.

It is a whole JBIG2 file (T.88 Annex D.1, sequential, with a file header): one page of 163 by 109 pixels, one generic region with
arithmetic coding. `tests/cli_pvec_pdf_151.rs` turns it into what a PDF stores (Annex D.3, embedded: no file header, no end-of-file
segment) and paints it on a page, so nothing derived is committed.

- bytes: 544
- sha256: `306b909c88b888f07d97ace0ca197b8b1544756f022684b3c7286476d6e26826`
