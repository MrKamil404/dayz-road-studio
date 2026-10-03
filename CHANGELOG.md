# Changelog

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
