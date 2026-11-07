# Changelog

## 2.5.0 — 2026-11-07

### Added
- **3D view** in Road Builder with a 2D / 3D switch: ASC terrain under the satellite image (or coloured by elevation), a transparent water plane at the water level, and the road and track P3D models with their PAA textures. Orbit, pan and zoom with the mouse; vertical exaggeration 1–5×.
- Terrain tiles in the 3D view follow the camera: full ASC and satellite detail near it, coarser with distance, built in the background for the visible area only, so the whole map is visible up to the horizon.
- Railway tracks: an ODOL reader measures the `Rail_Track`, `Rail_TrackE` and `Rail_TrackSmall` pieces; **Route across terrain** builds tracks from the real pieces with separate **Track constraints**; **Export tracks TB…** writes a Terrain Builder object import file.
- Level-crossing parts where a track crosses a road: `Rail_Track_Passing_25`, `Rail_Crossing_Block` decks or tiled small blocks depending on the track family.
- **Max. cut m**, **Max. fill m** and **Earthwork penalty** for roads and tracks: terrain routing may cut into or build up the terrain within those limits.
- **Side slope 1:** for grading; grading builds cut and embankment slopes outside the footprint.
- Update check against GitHub releases at start, with release notes, download, in-place install and restart. `DAYZ_NO_UPDATE_CHECK=1` skips the automatic check.
- P3D mesh reader (MLOD and ODOL) with texture coordinates, and a PAA reader for DXT1, DXT5 and ARGB textures.

### Changed
- **Grade ASC under the selected route** (formerly under the selected road) designs a profile within the maximum slope, cut and fill instead of following the terrain at part joints, and also grades tracks.

### Fixed
- Grading on coarse ASC grids no longer leaves a checkerboard of changed cells under the road: every cell that affects the road height is graded, and serpentine legs no longer share averaged cells.
- The satellite **Visibility** slider label is translated.

## 2.4.0 — 2026-11-06

### Added
- Water section under the **Forbidden** tool: terrain below a configurable water level (0 m by default) counts as forbidden, so terrain routing avoids the sea without drawing it.
- **Import water SHP…**: Polygon / PolygonZ / PolygonM rings become forbidden areas and PolyLine / PolyLineZ / PolyLineM parts become rivers of a chosen width. The import is undone as one step and saved in `.dzroad` projects; older projects load without water.
- **Show forbidden areas** toggle (also in the layer list): water below the level in blue, rivers as bands at their width, forbidden areas as red outlines. **Remove all forbidden areas** clears drawn and imported areas.

### Changed
- Forbidden areas, rivers and water are rasterised once over the ASC with a distance map, so routing stays fast with long rivers of many vertices. Terrain routes keep at least the route tolerance from every obstacle.
- Live drawing, **Fit models to points** and the TV4P export check apply the same forbidden areas and water as terrain routing.

## 2.3.0 — 2026-11-06

### Changed
- **Route across terrain** now searches the ASC slopes for the cheapest road that passes exactly through every route point, with a continuous heading through intermediate points. Depending on the terrain and settings the result can be serpentines, a loop around a hill, a detour or a nearly straight road.
- All **Road constraints** are applied during the search: maximum slope on every stretch, slope penalty on the total climb, routing step, minimum radius of every curve and route tolerance as the clearance from forbidden areas.
- Steep climbs are guided by a lower bound on road length (climb ÷ maximum slope) and a terrain cost map, so a point on a hilltop is reached by a longer road instead of failing.
- The terrain route is fitted with models without rounding its corners again; model fitting can step back and try alternative segments. The search keeps a 10 % slope margin first so the fitted models also satisfy the maximum slope.
- Route points stay unchanged and editable after routing; the fitted models carry the terrain route.
- Errors name the route point and the closest approach, or report when the road cannot leave a point in the direction it arrives from.

### Fixed
- Terrain routes no longer miss intermediate points by rounding corners after the search, and no longer ignore the minimum radius while searching.

## 2.2.0 — 2026-10-04

### Added
- Configurable terrain grading transition width, 0–500 metres beyond segment edges. Full grading remains inside the MLOD footprint; the outer band smoothly blends towards original terrain.
- Store the blending width in project files, with a 0 m default for older projects and support for undo/redo.
- Polish and English labels and regression coverage for transition strength, range boundaries, preserved source/NoData and saved settings.

## 2.1.2 — 2026-10-04

### Fixed
- TV4P export treats deletion of an already absent road as completed, allowing repeated exports against a base where that road has already been removed.
- Missing roads referenced by transforms or replacements still block export before modifying the destination.
- Regression tests cover mixed existing/absent deletions, repeated export and protection against missing edit targets.

## 2.1.1 — 2026-10-04

### Fixed
- Reduce unnecessary weaving during road fitting by scoring exit direction and avoiding curves that worsen alignment while the road is close to its target line.
- Limit lookahead to the end of the route and reserve space for the final cap, preventing artificial curls near the last point.
- Add regression coverage for straight and nearly straight routes with curve models available; retain rounded-corner fitting coverage.

## 2.1.0 — 2026-10-04

### Added
- Edit points mode for existing drawn project routes, including manual, live and imported SHP lines.
- Add points by double-clicking a line segment and remove points by double-right-clicking a handle, keeping at least two points.
- Select drawn lines along their segments, including routes without fitted models.
- Undo/redo for point insertion and removal. Point edits retain route identity and replacement bindings and invalidate fitted models so they can be regenerated.

### Fixed
- TV4P export and merging ignore metadata-like byte sequences inside the parsed road block, including segment IDs. This fixes false ambiguous metadata errors in projects such as balticrus2.

## 2.0.1 — 2026-10-03

- Set the maximum PNG width and height to 20480 pixels, supporting 20480 × 20480 exports.
- Update Polish and English dimension messages and documentation.
