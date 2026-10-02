# Terrain Builder Road Merger 1.1 — Rust

A Windows application for merging roads from `.tv4p` projects, with geometry previews, filtering, selection and PNG export. The current application is written in Rust. The old Python script is retained as reference material.

## Acknowledgements

Special thanks to **WoozyMasta** for [tv4p-road-tool](https://github.com/WoozyMasta/tv4p-road-tool). This program was made possible by his work.

## Version

Current application version: **1.1** (`1.1.0` in Cargo). The version is shown in the window title and can be printed with `tv4p_merge_roads.exe --version`.

## Recent changes

- Rewritten in Rust, with road part shapes read from MLOD P3D models.
- Separate road lists and previews for A, B and the result, with filtering and selection in the list and map.
- Export selected roads only, custom PNG resolution including 15360 × 15360, and background export.
- Full map export preserving road coordinates, with the bottom left corner at (200000, 0).
- **Polski / English** language selector covering the interface, tooltips, statuses, geometry warnings and CLI messages.
- The merge result marks roads added from B with a **NEW** label and green colour. A **New roads from B only** filter is available. Selected roads are yellow. These markers describe the current merge and survive model reloads, but are not stored in TV4P files.
- The default model folder is `P:\dz\structures\roads\parts`. Models in that folder take precedence over paths stored in the project. ODOL and other non-MLOD models produce a red message above the preview; hover over it to see filenames, or inspect road warnings. Invalid models are not replaced with filename-derived geometry.

## Application language

Change the language in the application's top bar. Existing statuses and warnings also change language without reloading the project. Polish is the default. Model names, paths, IDs and units stay unchanged. Standard controls in native file dialogs and operating-system error details use the Windows system language.

Use `--lang` before a command to choose the GUI or CLI language:

```powershell
.\tv4p_merge_roads.exe --lang en
.\tv4p_merge_roads.exe --lang pl
.\tv4p_merge_roads.exe --lang en inspect-p3d "P:\dz\structures\roads\parts\asf2_30 25.p3d"
```

The complete application translation catalog is in `src/translations.json`. JSON exports retain stable field names; warning text uses the selected language.

Run `python tools/check_translations.py` to audit catalog coverage and message placeholders.

## Getting started

Run `tv4p_merge_roads.exe`. Load projects A and B, choose the output file and merge the roads. The A, B and Result tabs have separate road lists, selections and view settings.

- Search by ID or road part names. Additional filters cover road type, length range and selected roads only.
- Click a row or road on the map to toggle selection. Selected roads are yellow. Select all visible roads or clear the selection using the corresponding buttons.
- Use the mouse wheel to zoom and drag to pan. Fit buttons cover visible or selected roads.
- Export roads visible after filtering or **selected roads only**, including selected roads hidden by a filter. The export scope shows its road count. Enter PNG width and height independently by clicking the number fields. Transparent backgrounds are supported.
- Default export covers the **full 15360 × 15360 m map**, with the bottom left corner at **E=200000, N=0**, producing a **15360 × 15360 px** image. Map size, map origin and image resolution are separate settings. These defaults give one pixel per metre.
- Full map exports preserve road placement: `x=(E−E0)×pngWidth/mapWidth`, `y=pngHeight−(N−N0)×pngHeight/mapHeight`. North is at the top. There is no margin or framing around the selection. Geometry outside the map is clipped. Disable the full map area to fit the exported roads instead.
- PNG dimensions can be 128–32768 px per side, with at most 268,435,456 pixels in total. A 15360 × 15360 image requires about 900 MiB for its RGBA buffer plus additional memory during saving. Export runs in the background without blocking the interface.

## Road geometry

Default model folder: `P:\dz\structures\roads\parts`. Change it in the application if needed. The reader uses the lowest visual LOD mesh in **MLOD P3D** files and the `LB/PB`, `LE/PE`, `LH/LD`, `PH/PD` memory points. The X/Z projection provides the actual footprint, and connection points determine the next part's placement. ODOL files are not supported.

A road contains a key part, its position and rotation, and separate branch chains. Rotation is read from `0x8C` in degrees using Terrain Builder's rotation direction. `0x92` extends the key part's end, `0x93` its beginning, and `0x94`/`0x95` its side connections. Left bends connect through the opposite end of the model. These field interpretations come from analysing TV4P data and models, rather than a published format specification.

When a model file is missing, the reader derives the path from its filename: length for a straight part, angle and radius for a bend. Names `6` and `12` represent 6.25 m and 12.5 m; `0 2000` represents a 0.5° bend. Arc length equals `radius × angle in radians`. Unknown names and missing connectors produce warnings rather than arbitrary substitute geometry. Row tooltips show the numbers of MLOD and filename-derived parts. Road length includes the key part and all branches.

## Merging projects

The merger checks road type and junction definitions (`0x88`, `0x89`), skips identical roads, allocates unique IDs and rebuilds the road block. Other data comes from A. A modified or moved road may be added as a separate road. The output path must differ from both inputs. Open the result in Terrain Builder before using it in your map project.

## Command line

```powershell
.\tv4p_merge_roads.exe --lang en merge A.tv4p B.tv4p output.tv4p
.\tv4p_merge_roads.exe --lang en png map.tv4p roads.png
.\tv4p_merge_roads.exe --lang en export map.tv4p roads.json
.\tv4p_merge_roads.exe --lang en roundtrip map.tv4p
.\tv4p_merge_roads.exe --lang en types map.tv4p
.\tv4p_merge_roads.exe --lang en inspect-p3d "P:\dz\structures\roads\parts\asf2_30 25.p3d"
```

`png` and `export` accept an optional model folder as their last argument. CLI PNG export covers all roads at 2048 × 1536 px.

## Building

Requires Rust and Windows compilation tools:

```powershell
cargo build --release
```

Executable: `target\release\tv4p_merge_roads.exe`.
