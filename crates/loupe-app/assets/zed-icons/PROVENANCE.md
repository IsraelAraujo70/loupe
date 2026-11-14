# Icons from Zed

These SVGs are copied unchanged from the Zed editor
(https://github.com/zed-industries/zed), commit
`f8cfc7ed9e57503def9479138894f3139f5400cc` (2026-11-10):

- `check.svg`, `dash.svg` from `assets/icons/`
- every other file from `assets/icons/file_icons/`

Licenses, as Zed states them:

- `assets/icons/LICENSES` grants the ISC License (Lucide, with portions
  under MIT from Feather) for Zed's Lucide-derived icons. That covers the
  generic glyphs used here: chevrons, folders, file, check, dash.
- The language and tool logos in `assets/icons/file_icons/` (rust, docker,
  typescript, …) have no license of their own, so they fall under the Zed
  repository's default, GPL-3.0-or-later. Those files keep that license;
  section 13 of the GPL-3.0 allows combining them with Loupe, which is
  licensed under the AGPL-3.0.

Only the SVG files are copied. The code that maps file names to these icons
(`crates/loupe-app/src/assets.rs`) is Loupe's own; Zed's `theme`,
`file_icons`, `ui`, and `project_panel` crates (GPL-3.0-or-later) were read
for behavior only.
