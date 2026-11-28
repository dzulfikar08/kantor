# VBA project fixtures

Real Excel-made VBA projects (`xl/vbaProject.bin` taken out of the workbook
unchanged), from the test data of Apache POI, licensed under the Apache
License 2.0 (https://www.apache.org/licenses/LICENSE-2.0). Apache POI,
copyright The Apache Software Foundation (https://poi.apache.org/).

| File | Taken from | SHA-256 of the source workbook |
|---|---|---|
| `poi-SimpleMacro.vbaProject.bin` | https://github.com/apache/poi/blob/trunk/test-data/spreadsheet/SimpleMacro.xlsm | `f76c986f4ebc25c2cc57c088b2511a1269f4bd61d6223a2ab58db351da348ba6` |
| `poi-testNames.vbaProject.bin` | https://github.com/apache/poi/blob/trunk/test-data/spreadsheet/testNames.xlsm | `2283c7bbffb916e9764b3dd0338a48f2a8b1685e6acc0e93482c3433578fd85f` |

`poi-SimpleMacro`: Excel 2010 (Office 14 references), code page 1252, a
standard module `Module1` with one macro carrying a description and a
shortcut attribute, and the document modules `ThisWorkbook`, `Sheet1`–`Sheet3`.

`poi-testNames`: Excel 2013 (Office 15), code page 1252, `Module1` with a
macro and two functions, a control reference (MSForms) with an original
library record.

Everything else (class modules, UserForms, code pages 1251 and 932, long
modules, missing streams) is written by `kantor_vba::synth::Builder` in the
tests, not stored as binary files.

## UserForms

Two real Excel-made workbooks with a UserForm, from public GitHub
repositories under the MIT licence (the licence text is in each repository's
`LICENSE`; `xl/vbaProject.bin` taken out unchanged):

| File | Taken from | Licence | SHA-256 of the source workbook |
|---|---|---|---|
| `hriday-userform.vbaProject.bin` | https://github.com/HridayBegins/userform/blob/4d5bc599f402dbbc5412479893311a23a7d1a8a0/userform.xlsm | MIT, Copyright (c) 2026 Hriday | `af317252a7b41a473a1a8b7c780c1065f3825495f0575a6d4b1123313708013b` |
| `ayliinkaya-StudentRegistration.vbaProject.bin` | https://github.com/ayliinkaya/ExcelVBA_userform_StudentRegistration/blob/4c37895a7e8eecfcc39052f891d64b9b4261aaf6/Project_VBA.xlsm | MIT, Copyright (c) 2021 ayliinkaya | `80f4aed794e43b01957fc75edf5e3ca0dad561b1658c6469841b057b7d9b138a` |

`hriday-userform`: `UserForm4` (code page 1252) with three Labels (white
bold Yu Gothic UI on red), three TextBoxes and two CommandButtons (`ADD`,
`RESET`); the form's back colour is set.

`ayliinkaya-StudentRegistration`: `KayitFormu` (code page 1254, Turkish
captions stored as UTF-16) with Labels, TextBoxes, four CommandButtons and two
ComboBoxes whose `RowSource` is `VERİ!LABNO` / `VERİ!SINIFNO`. The whole
workbook is also in `crates/sheets_script/tests/fixtures/` (see the README
there) for running the form.

Other designer layouts (Frames, MultiPages, ListBoxes, pictures…) are written
by `kantor_vba::forms::write_streams` through `synth::Builder::form`.
