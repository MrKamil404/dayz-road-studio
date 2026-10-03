# Changelog

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
