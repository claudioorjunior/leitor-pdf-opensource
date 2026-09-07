# PDF stack research (Tsuro, 2026-09-07)

Question: is `pdfium-render` still the right engine, or should Tsuro switch?
Requirements: page raster render, text extraction WITH glyph positions (selection + search),
fast open of large PDFs, macOS + Windows distribution, MIT license kept.

## Comparison

| Engine | License | Render | Text + positions | Ship burden | Maintenance |
|---|---|---|---|---|---|
| pdfium-render 0.8 (current) | Crate MIT/Apache; Pdfium itself BSD-3 (permissive) | Yes, Chrome-grade fidelity | Yes (`FPDFText_*`, per-char boxes; already used by `session.rs`) | Pdfium NOT bundled, app must ship the dynamic lib or static-link per platform | Active: 0.8.27 on crates.io, upstream commit Aug 2026 |
| hayro 0.7.1 (pure Rust) | MIT/Apache | Yes, CPU rasterizer, but self-described as far from feature-complete, no perf work yet | No text-extraction API (render to bitmap/SVG only) | None (pure Rust, `forbids unsafe`) | Very active (commit Aug 24 2026, 1780 commits) but not ready |
| mupdf-rs / mupdf-sys | AGPL-3.0 (MuPDF is AGPL/commercial dual) | Yes, fastest reputation | Yes | Compiles ~1.2M SLoC C (13.8 MiB crate); heavy build | Active (commit Aug 18 2026) |
| poppler-rs | GPL (libpoppler) + system cairo/glib deps | Yes | Yes | Needs system Poppler on macOS AND Windows, painful installers | Niche, low activity |
| PDFKit/Quartz | Apple proprietary | Yes, macOS only | Yes, macOS only | None on Mac, impossible on Windows | N/A |
| `pdf` crate (pure Rust) | Permissive | No renderer | Parse/text only | None | N/A |

## Notes per candidate

**pdfium-render (keep).** Idiomatic high-level bindings to Pdfium, the Chromium PDF
engine. Crate is MIT/Apache licensed; Pdfium itself is BSD-3, so MIT + closed
distribution stay fine. Shipped-version 0.8.27, maintainer active in Aug 2026.
The one real cost: the crate does not include Pdfium, so releases must bundle
the per-platform dynamic lib (or static-link). That is a packaging chore, not a
dealbreaker, and Tauri already ships native libs per platform.
Sources: https://crates.io/crates/pdfium-render, https://github.com/ajrcarey/pdfium-render

**hayro (watch, do not adopt).** Pure-Rust renderer from the Typst ecosystem
(LaurenzV), MIT/Apache, no native lib to ship, `forbids unsafe`. But version 0.7.1
(Aug 28 2026) states outright it is far from feature-complete with no performance
work yet, and it exposes rendering only, no text-with-positions API. Adopting it
today means losing selection/search and rewriting them from scratch later.
Revisit in 6-12 months.
Sources: https://docs.rs/hayro, https://github.com/LaurenzV/hayro

**MuPDF (rejected on license).** Technically the best engine (speed + text), but
AGPL-3.0 bindings and AGPL/commercial-dual engine. Using it forces Tsuro to go
AGPL or buy a commercial license from Artifex. Incompatible with the MIT goal.
Sources: https://github.com/messense/mupdf-rs, https://mupdf.readthedocs.io/en/1.27.0/license.html, https://artifex.com/licensing

**poppler-rs (rejected).** GPL engine plus system cairo/glib dependency makes
Windows/macOS installers miserable for zero gain over Pdfium.
Source: https://github.com/DMSrs/poppler-rs

**PDFKit (rejected).** macOS-only; Tsuro targets Windows too (NSIS/MSI).
**`pdf` crate (rejected).** Parser only, no rasterizer.

## Verdict

Keep pdfium-render. The open-without-stall problem is Tsuro's architecture
(reload + full-catalog extract per open), not the engine: Pdfium itself opens
lazily per page. The fix already in progress (open extracts page 0 only,
per-page `PageData` on demand; next: hold one `PdfDocument` open instead of
`with_doc` per call) is the correct and smallest path. No stack switch.

Follow-up: the `with_doc`-per-call reload is gone. `PdfiumEngine` now holds
one `PdfDocument` open on a dedicated worker thread (bind + parse once per
file), serves `PageData`/`Render` over channels, and the session runs all
blocking work via `spawn_blocking`.
