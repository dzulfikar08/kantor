# slides_engine data

## presetShapeDefinitions.xml

The DrawingML preset shape definitions (187 shapes: guides, adjust values,
handles, connection sites, text rectangle and paths) from **ECMA-376 Office
Open XML File Formats, Part 1, Annex D** (the `OfficeOpenXML-DrawingMLGeometries`
archive that accompanies the standard). Unmodified, byte for byte as
redistributed by Apache POI
(`poi/src/main/resources/org/apache/poi/sl/draw/geom/presetShapeDefinitions.xml`,
fetched 2026-11-10, 538 970 bytes); LibreOffice, python-pptx tooling and
others ship the same file.

`build.rs` compacts it at build time (guides, text rectangle and paths only,
about 110 KB) and `src/geom/presets.rs` parses that on first use.

### Licence

ECMA-376 is published by Ecma International free of charge. Its copyright
notice permits reproduction:

> © Ecma International. This document and translations of it may be copied
> and furnished to others, and derivative works that comment on or otherwise
> explain it or assist in its implementation may be prepared, copied,
> published, and distributed, in whole or in part, without restriction of
> any kind, provided that the above copyright notice and this section are
> included on all such copies and derivative works. However, this document
> itself may not be modified in any way, including by removing the copyright
> notice or references to Ecma International, except as needed for the
> purpose of developing any document or deliverable produced by Ecma
> International.
>
> The limited permissions granted above are perpetual and will not be
> revoked by Ecma International or its successors or assigns.
>
> This document and the information contained herein is provided on an "AS
> IS" basis and ECMA INTERNATIONAL DISCLAIMS ALL WARRANTIES, EXPRESS OR
> IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTY THAT THE USE OF THE
> INFORMATION HEREIN WILL NOT INFRINGE ANY OWNERSHIP RIGHTS OR ANY IMPLIED
> WARRANTIES OF MERCHANTABILITY OR FITNESS FOR A PARTICULAR PURPOSE.

The file is kept unmodified; the compacted form built from it is a
derivative that assists in implementing the standard.
