# slides_engine data

## presetShapeDefinitions.xml

The DrawingML preset shape definitions (187 shapes: guides, adjust values,
handles, connection sites, text rectangle and paths) from **ECMA-376 Office
Open XML File Formats, Part 1, Annex D** (`OfficeOpenXML-DrawingMLGeometries.zip`
in the standard's download), in the corrected copy that Apache POI ships
(`poi/src/main/resources/org/apache/poi/sl/draw/geom/presetShapeDefinitions.xml`,
fetched 2026-11-10, 538 970 bytes). POI's copy is reformatted and fixes errata
in Ecma's file, e.g. Ecma's second `upDownArrow` is really `upArrow`, and
the guide `2dang` (a name starting with a digit) is renamed `dang2`. The
file is committed exactly as POI has it.

`build.rs` compacts it at build time (guides, text rectangle and paths only,
about 110 KB) and `src/geom/presets.rs` parses that on first use.

### Licence

- **The definitions** are part of ECMA-376, which Ecma International
  publishes free of charge under its default copyright licence
  (<https://ecma-international.org/policies/by-ipr/ecma-text-copyright-policy/>).
  Among the permitted derivative works it lists use of the specification "in
  standard conformant products by implementing (e.g. by copy and paste
  wholly or partly) the functionality therein", which is what Kantor does.
  The standard is provided "AS IS" and Ecma disclaims all warranties.
- **POI's corrections** are part of Apache POI, licensed under the Apache
  License 2.0 (<https://www.apache.org/licenses/LICENSE-2.0>), which is
  compatible with Kantor's AGPL-3.0-or-later.

`build.rs` compacts the file at build time; the compacted form is used only
to draw the shapes.
