# Terrain Builder Road Merger

A small Windows utility for merging roads from two Terrain Builder `.tv4p` projects. It has a graphical interface and a command-line mode, and can also export road details to JSON.

![Terrain Builder Road Merger icon](app_icon.png)

## Features

- Add roads from a donor project to a base project.
- Skip roads that already exist in the base.
- Check that both projects have matching Road Tool road types and junction definitions.
- Export road details to JSON, compare road types, and check whether the road block can be rebuilt byte-for-byte.
- Preserve the rest of the base project. Objects and other non-road data from the donor are not merged.

## Run the application

### Windows executable

Run `tv4p_merge_roads.exe` to open the graphical interface. Choose project A (the base), project B (the road donor), and an output file, then click **Merge roads**. The application icon is included in the executable and shown in the window title bar.

### Python source

The script uses Python's standard library and Tkinter. Start the graphical interface with:

```powershell
python tv4p_merge_roads.py
```

## Merge workflow

1. Start each contributor from the latest shared base project.
2. Add new roads only. Avoid moving or editing existing roads; a moved road is treated as a new road and may be duplicated.
3. Keep Road Tool road types and junction definitions unchanged while contributors are working. The merge checks these settings (blocks `0x88` and `0x89`) and stops if they differ.
4. Merge the projects, make a backup of the previous base, and open the output in Terrain Builder.
5. Verify the roads in Road Tool, along with the project's layers and rasters. Export the road list if you want to check the resulting count.

Project A is the base and supplies the entire non-road project data, including objects and layers. Project B supplies roads that are not already in A. The merge compares road contents without their internal IDs, then assigns new unique IDs to imported roads and parts.

## Command-line usage

Run commands from the folder containing the files. Use `python` with the script or `tv4p_merge_roads.exe` in place of the executable name.

```powershell
# Merge B's new roads into A and write a separate output project
python tv4p_merge_roads.py merge A.tv4p B.tv4p output.tv4p

# Export road details and a project summary to JSON
python tv4p_merge_roads.py export output.tv4p roads.json

# Print road type and junction definitions for comparison
python tv4p_merge_roads.py types output.tv4p

# Check whether the road list can be parsed and rebuilt byte-for-byte
python tv4p_merge_roads.py roundtrip output.tv4p
```

The JSON export contains a project summary (road and part counts and map-coordinate bounds) and a list of roads with their IDs, lengths, start points, models, and part counts.

## Build the Windows executable

With Python and `uv` installed, build a one-file executable with PyInstaller:

```powershell
uv run --with pyinstaller -- python -m PyInstaller --noconfirm --clean --onefile --name tv4p_merge_roads --icon app_icon.ico --add-data "app_icon.ico;." tv4p_merge_roads.py
```

The executable is written to `dist/tv4p_merge_roads.exe`. The `.ico` resource sets the Windows executable icon and is bundled for the Tkinter window icon. The build keeps a console so command-line output remains available.

## Limitations

- The tool merges road data only; donor objects and other non-road changes are not included.
- The `.tv4p` format is not officially documented.
- Always keep a backup and verify each merged project in Terrain Builder before sharing it as the new base.
