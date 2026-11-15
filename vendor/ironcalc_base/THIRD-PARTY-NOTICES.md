# Third-party notices

## formulon (FORECAST.ETS)

`src/functions/kantor_ets/` is a Rust port of parts of formulon
(https://github.com/libraz/formulon): `src/eval/forecast_ets_lazy.cpp`,
`src/eval/holt_winters.cpp`, `src/eval/ets_series.cpp` and the normal
quantile of `src/eval/stats/special_functions.cpp`. Copyright libraz and the
formulon contributors. Licensed under the Apache License, Version 2.0
(http://www.apache.org/licenses/LICENSE-2.0, the same text as
`LICENSE-Apache-2.0` in this directory). Changes: translated to Rust and
adapted to IronCalc's values and errors (KANTOR_PATCHES.md §25).

## Unicode CLDR locale data

`src/locale/locales.bin` (and its human-readable form,
`generate_locale/locales.json` in the repository) contains locale data
(month and weekday names, date, number and currency formats, currency
symbols) derived from the Unicode Common Locale Data Repository (CLDR),
taken from the `cldr-json` distribution version 48.2.0 (CLDR 48.2):
https://github.com/unicode-org/cldr-json

The data is used and redistributed under the Unicode License v3
(SPDX: Unicode-3.0). This notice must accompany any redistribution of
that data, including binaries built from this crate:

```text
UNICODE LICENSE V3

COPYRIGHT AND PERMISSION NOTICE

Copyright © 2015-2024 Unicode, Inc.

NOTICE TO USER: Carefully read the following legal agreement. BY
DOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING DATA FILES, AND/OR
SOFTWARE, YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE
TERMS AND CONDITIONS OF THIS AGREEMENT. IF YOU DO NOT AGREE, DO NOT
DOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE THE DATA FILES OR SOFTWARE.

Permission is hereby granted, free of charge, to any person obtaining a
copy of data files and any associated documentation (the "Data Files") or
software and any associated documentation (the "Software") to deal in the
Data Files or Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, and/or sell
copies of the Data Files or Software, and to permit persons to whom the
Data Files or Software are furnished to do so, provided that either (a)
this copyright and permission notice appear with all copies of the Data
Files or Software, or (b) this copyright and permission notice appear in
associated Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
THIRD PARTY RIGHTS.

IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS NOTICE
BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL DAMAGES,
OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS,
WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THE DATA
FILES OR SOFTWARE.

Except as contained in this notice, the name of a copyright holder shall
not be used in advertising or otherwise to promote the sale, use or other
dealings in these Data Files or Software without prior written
authorization of the copyright holder.

SPDX-License-Identifier: Unicode-3.0
```
