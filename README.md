# DayZ Road Studio 2.0

A Windows application for merging and designing roads in Terrain Builder `.tv4p` projects. DayZ Road Studio combines **Merge** and **Road Builder** with geometry previews, terrain tools and PNG export. Written in Rust.

The same executable now includes **Road Builder**, integrated from DayZRoadToolExternal 0.1.5. Starting without a CLI command opens a launcher with **Merge** and **Road Builder**. After choosing a toolset, use the top tabs to switch; both tools retain their loaded projects, views, selections and work in progress for the current session. **Tools** returns to the launcher. The shared **Polski / English** selector covers both tools, including Builder dialogs and statuses. Background jobs continue while another tab is active. Closing checks Builder's unsaved changes even from Merge.

Road Builder includes editing existing drawn route lines, manual and live road drawing, MLOD segment fitting, SHP import, tiled satellite BMP/PNG, ASC terrain and contours, terrain routing, grading under road footprints, undo/redo, `.dzroad` project saving and TV4P/ASC export. See [the Road Builder guide](docs/road-builder.md). To transfer work between tools, export a TV4P file and open it in the other tool; switching tabs does not transfer projects automatically. Sessions are not restored automatically after restarting.

The Builder sources are included in `crates/road-builder`; building needs no neighbouring repository or separate Builder EXE. Its TV4P and geometry implementations remain separate from Merge to preserve the original editor's export behaviour.

![Launcher](https://raw.githubusercontent.com/MrKamil404/dayz-road-studio/refs/heads/main/images/prev2.png)
![Legacy Road Merger 1.1 interface](https://raw.githubusercontent.com/MrKamil404/Terrain-Builder-Road-Merger/refs/heads/main/images/prev1.png)
![Road builder](https://raw.githubusercontent.com/MrKamil404/dayz-road-studio/refs/heads/main/images/prev3.png)
![Result](https://raw.githubusercontent.com/MrKamil404/dayz-road-studio/refs/heads/main/images/prev4.png)

> [!CAUTION]
> Always keep a separate backup copy of your Terrain Builder project before merging roads or saving a result. Keep the original input projects and write the merged result to a different file.

## Acknowledgements

Special thanks to **WoozyMasta** for [tv4p-road-tool](https://github.com/WoozyMasta/tv4p-road-tool). This program was made possible by his work.

## Version

Current application version: **2.1.2** (`2.1.2` in Cargo). The application is named **DayZ Road Studio**. The executable remains `tv4p_merge_roads.exe` for compatibility with existing workflows. The version is shown in the window title and can be printed with `tv4p_merge_roads.exe --version`.

## Changes in 2.0

- Launcher and persistent Merge / Road Builder tabs in one executable.
- Full Road Builder integration: live and manual drawing, SHP, satellite BMP/PNG, ASC, terrain routing and grading, undo/redo and `.dzroad` projects.
- Shared dark theme and Polish / English interface across both toolsets.
- Background PNG export from Builder, including map-aligned images and transparency.
- Configurable road type colors in both tools, respected by PNG export and saved in Builder projects.
- Background jobs remain active across tab changes; closing checks unsaved Builder changes from any tab.

### Earlier Merge features

- Rewritten in Rust, with road part shapes read from MLOD P3D models.
- Separate road lists and previews for A, B and the result, with filtering and selection in the list and map.
- Export selected roads only, custom PNG resolution including 15360 × 15360, and background export.
- Full map export preserving road coordinates, with the bottom left corner at (200000, 0).
- **Polski / English** language selector covering the interface, tooltips, statuses, geometry warnings and CLI messages.
- The merge result marks roads added from B with a **NEW** label and green colour. A **New roads from B only** filter is available. Selected roads are yellow. These markers describe the current merge and survive model reloads, but are not stored in TV4P files.
- The default model folder is `P:\dz\structures\roads\parts`. Models in that folder take precedence over paths stored in the project. ODOL and other non-MLOD models produce a red message above the preview; hover over it to see filenames, or inspect road warnings. Invalid models are not replaced with filename-derived geometry.

## Application language

Change the language using the selector in the top navigation bar of the application. Existing statuses and warnings also change language without reloading the project. Polish is the default. Model names, paths, IDs and units stay unchanged. Standard controls in native file dialogs and operating-system error details use the Windows system language.

Use `--lang` before a command to choose the GUI or CLI language:

```powershell
.\tv4p_merge_roads.exe --lang en
.\tv4p_merge_roads.exe --lang pl
.\tv4p_merge_roads.exe --lang en inspect-p3d "P:\dz\structures\roads\parts\asf2_30 25.p3d"
```

The complete application translation catalog is in `src/translations.json`. JSON exports retain stable field names; warning text uses the selected language.

Run `python tools/check_translations.py` to audit catalog coverage and message placeholders. Python is optional and is only needed for this development utility; running or building the application does not require it.

## GUI workflow

### Merge projects

1. Back up both input projects. Use **A** as the main project and **B** as the road donor. Both projects must have matching Road Tool road type and junction definitions.
2. Run `tv4p_merge_roads.exe` without a command and choose **Merge** in the launcher. For the English interface, run `.\tv4p_merge_roads.exe --lang en`, or use the language selector in the top navigation bar.
3. Check the **MLOD** folder in the top bar. Its default is `P:\dz\structures\roads\parts`. Use the `…` button beside this field to choose a folder, or edit the path and click **Apply**. Review any model warnings.
4. Click **A…** to load the main project, then **B…** to load the donor. Use **File A** and **File B** to inspect their road lists and previews.
5. Enter an output path in **Output**, or use the `…` button beside it (**Save as** in the tooltip). Choose a file different from both inputs.
6. Click **Merge roads**. On success, the application switches to **Result** and shows the merge counts. Roads actually added from B have a **NEW** label and are green; roads skipped as duplicates are not marked as new.
7. Inspect the result. Use **New roads from B only** to isolate added roads, then optionally **Select visible** to select them. Selected roads are yellow.
8. Open the saved result in Terrain Builder and check the roads and the rest of the project before adopting it as your new main project. All non-road content comes from A.

> [!NOTE]
> If merging reports incompatible Road Tool settings, resolve the definition differences in Terrain Builder before trying again. NEW markers describe the current GUI merge; they are not stored in the saved TV4P file.

### Export a PNG

1. Choose **File A**, **File B** or **Result**. Export uses the active tab, so loading both inputs or merging is not required just to preview or export one project.
2. Filter the road list if needed. Click rows or roads on the map to select them, or click **Select visible**.
3. In the bottom bar, choose **Selected roads only** or **Visible after filtering** from the PNG scope menu. Its compact label is **Selected** or **Visible**. Selected scope includes selected roads hidden by filters; visible scope includes all roads that pass the current filters.
4. Set the two **px** fields to the image width and height. The **15360²** button sets both to 15360. Enable **Alpha** for a transparent background.
5. Enable **Map** to preserve placement on the full map. Set **E** and **N** to the bottom-left world coordinates and the two **m** fields to the map width and height. For the default map, use E=200000, N=0 and 15360 × 15360 m. Disable **Map** to fit the exported roads instead.
6. Check the road count beside the export button. **Export PNG** is enabled only when this scope contains roads and another export is not running. Choose the PNG output file and wait for the success message; the large-image export runs in the background.

> [!NOTE]
> Image resolution and map size are separate settings. PNG 15360 × 15360 px over a 15360 × 15360 m map gives 1 pixel per metre. Map export clips geometry outside the configured map bounds.

### Interface reference

The A, B and Result tabs have separate road lists, selections and view settings.

Project controls remain at the top, the road list on the left, the preview in the centre and PNG export settings at the bottom. Both control bars are compact and keep their settings on one line when there is enough space; settings wrap on narrower windows. The language selector stays in the top navigation bar. Short labels and buttons provide full descriptions in tooltips. Both toolsets use the same dark theme, blue roads and yellow selections.

- Search by ID or road part names. Additional filters cover road type, length range and selected roads only.
- Click a row or road on the map to toggle selection. Selected roads are yellow. Select all visible roads or clear the selection using the corresponding buttons.
- Use the mouse wheel to zoom and drag to pan. Fit buttons cover visible or selected roads.
- Export roads visible after filtering or **selected roads only**, including selected roads hidden by a filter. The export scope shows its road count. Enter PNG width and height independently by clicking the number fields. Transparent backgrounds are supported.
- Default export covers the **full 15360 × 15360 m map**, with the bottom left corner at **E=200000, N=0**, producing a **15360 × 15360 px** image. Map size, map origin and image resolution are separate settings. These defaults give one pixel per metre.
- Full map exports preserve road placement: `x=(E−E0)×pngWidth/mapWidth`, `y=pngHeight−(N−N0)×pngHeight/mapHeight`. North is at the top. There is no margin or framing around the selection. Geometry outside the map is clipped. Disable the full map area to fit the exported roads instead.
- PNG dimensions can be 128–20480 px per side, with at most 419,430,400 pixels in total. A 15360 × 15360 image requires about 900 MiB for its RGBA buffer plus additional memory during saving. Export runs in the background without blocking the interface.

## Road geometry

Default model folder: `P:\dz\structures\roads\parts`. Change it in the application if needed. The reader uses the lowest visual LOD mesh in **MLOD P3D** files and the `LB/PB`, `LE/PE`, `LH/LD`, `PH/PD` memory points. The X/Z projection provides the actual footprint, and connection points determine the next part's placement. ODOL files are not supported.

> [!NOTE]
> Road previews require unbinarized MLOD P3D models. ODOL or other non-MLOD files produce a warning. A missing file may use filename-derived geometry, but an existing file in an unsupported format is not silently replaced.

A road contains a key part, its position and rotation, and separate branch chains. Rotation is read from `0x8C` in degrees using Terrain Builder's rotation direction. `0x92` extends the key part's end, `0x93` its beginning, and `0x94`/`0x95` its side connections. Left bends connect through the opposite end of the model. These field interpretations come from analysing TV4P data and models, rather than a published format specification.

When a model file is missing, the reader derives the path from its filename: length for a straight part, angle and radius for a bend. Names `6` and `12` represent 6.25 m and 12.5 m; `0 2000` represents a 0.5° bend. Arc length equals `radius × angle in radians`. Unknown names and missing connectors produce warnings rather than arbitrary substitute geometry. Row tooltips show the numbers of MLOD and filename-derived parts. Road length includes the key part and all branches.

## Merging projects

> [!NOTE]
> **Project A is the main (base) project. Project B contributes roads only.** The result keeps A's existing roads and adds roads from B that are not already present. All other project content comes from A; objects, layers, rasters and other non-road content from B are not imported.

The merger checks road type and junction definitions (`0x88`, `0x89`), skips identical roads, allocates unique IDs and rebuilds the road block. A modified or moved road may be added as a separate road. The output path must differ from both inputs. Open the result in Terrain Builder before using it in your map project.

## Command line

### CLI workflow

Run these PowerShell commands from the directory containing the executable. Replace the example filenames with your actual projects. Back up the inputs first, and choose an existing output directory.

1. Check the program version and available syntax. Optionally inspect both projects' Road Tool definitions; the merge command also checks their compatibility automatically.

```powershell
.\tv4p_merge_roads.exe --lang en --version
.\tv4p_merge_roads.exe --lang en --help
.\tv4p_merge_roads.exe --lang en types A.tv4p
.\tv4p_merge_roads.exe --lang en types B.tv4p
```

2. Merge donor B's roads into base A. The output must differ from both inputs. Stop if the command fails; do not continue exporting a result left from an earlier run.

```powershell
.\tv4p_merge_roads.exe --lang en merge A.tv4p B.tv4p output.tv4p
if ($LASTEXITCODE -ne 0) { throw "Merge failed; check the error above." }
```

3. Optionally check that the result's road block can be rebuilt byte-for-byte, then export road geometry to JSON and create an overview PNG. Pass a model folder as the last argument to `export` and `png`, or omit it to use the default folder.

```powershell
.\tv4p_merge_roads.exe --lang en roundtrip output.tv4p
if ($LASTEXITCODE -ne 0) { throw "Road block roundtrip failed." }

.\tv4p_merge_roads.exe --lang en export output.tv4p roads.json "P:\dz\structures\roads\parts"
if ($LASTEXITCODE -ne 0) { throw "JSON export failed." }

.\tv4p_merge_roads.exe --lang en png output.tv4p roads.png "P:\dz\structures\roads\parts"
if ($LASTEXITCODE -ne 0) { throw "PNG export failed." }
```

4. Review the reported geometry warnings and exported overview. To diagnose an individual road part, inspect its MLOD model:

```powershell
.\tv4p_merge_roads.exe --lang en inspect-p3d "P:\dz\structures\roads\parts\asf2_30 25.p3d"
```

5. Open `output.tv4p` in Terrain Builder and check the result before replacing your main project. `roundtrip` checks the road block only; it is not a validation of the entire project or its geometry.

> [!NOTE]
> CLI `png` exports **all roads at 2048 × 1536 px**, with an opaque background and a frame fitted to the roads. CLI currently has no options for road selection, filtering, custom resolution, transparent background or fixed map bounds. Use the GUI for selected-road exports and map-aligned PNGs such as 15360 × 15360.

> [!CAUTION]
> Output and export commands can overwrite existing destination files. Keep backups and use fresh output filenames. Successful geometry export can still report warnings and contain an incomplete preview; inspect those warnings before relying on it.

### Command reference

Put optional `--lang en` or `--lang pl` **before** the command. Exit code 0 indicates success; errors return exit code 1. Run without a command to open the GUI.

| Command | Arguments | Purpose |
| --- | --- | --- |
| `--version` / `-V` | None | Print the application version. |
| `--help` / `help` | None | Show CLI syntax. |
| `merge` | `A.tv4p B.tv4p output.tv4p` | Add B's roads to A; keep other content from A. |
| `types` | `input.tv4p` | List road type and junction definition entries. |
| `roundtrip` | `input.tv4p` | Check byte-for-byte rebuilding of the road block without modifying the input. |
| `export` | `input.tv4p output.json [models-folder]` | Export all roads and their geometry to JSON. |
| `png` | `input.tv4p output.png [models-folder]` | Export an overview PNG of all roads. |
| `inspect-p3d` | `model.p3d` | Read model length, mesh triangle count and connection ports. |

## Building

Requires Rust and Windows compilation tools:

```powershell
cargo build --release
```

Executable: `target\release\tv4p_merge_roads.exe`.

Run tests and lint checks for both toolsets with `cargo test --workspace` and `cargo clippy --workspace --all-targets -- -D warnings`. `python tools/check_translations.py` audits both translation catalogs. The optional `ui-screenshot` feature supports the eframe development screenshot mechanism; normal builds do not require it.

Road Builder also exports road geometry to PNG from its bottom bar. Choose all roads or the selected road, dimensions, alpha and map bounds. Map mode uses the origin and size in the Builder project settings; disabling it fits the image to the exported roads. Existing roads include current moves/rotations and exclude deleted/replaced roads. Generated segments are included; unfinished sketches, satellite and contours are excluded. PNG jobs continue while switching tabs.

Both left panels include **Road type colors**. Click a type swatch to choose its RGB color; **Reset** restores the default. Merge shares its palette across A, B and Result for the current session. Builder stores its palette in `.dzroad` projects, including compatibility with older projects that have no palette. Custom type colors override yellow selection and green NEW markers in PNG; preview selections remain yellow and NEW labels remain available in Merge. Unconfigured types retain their existing colors. Palettes do not alter TV4P files.

### Edit an existing drawn route

Select a route on the map or in the project route list, then choose **Edit points**. Drag a point to reshape the line. Double-click a line segment to insert a point, or double-right-click a point to remove it (at least two points remain). You can select unfinished lines by clicking anywhere on the line. Drag away from handles to move the entire route. Point edits clear the previous fitted segments; use **Fit models to points** again before TV4P export. Undo/redo restores both the line and its fitted segments, and point edits are saved in the .dzroad project. This edits project route lines; imported TV4P roads retain their move/rotate controls.

See [CHANGELOG.md](CHANGELOG.md) for release changes.
