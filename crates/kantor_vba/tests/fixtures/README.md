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
