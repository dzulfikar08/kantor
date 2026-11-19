#!/usr/bin/env bash
# macos-info-plist.sh VERSION  ->  Kantor.app's Info.plist on stdout.
# Used by macos-app.sh; scripts/check-file-types.py checks its output in CI.
#
# Document types are declared by UTI (LSItemContentTypes), which is what
# Finder's Open With, double-clicks and Dock drops go by; extensions alone
# (CFBundleTypeExtensions) are deprecated and ignored when UTIs are given.
# LSHandlerRank Alternate: Kantor is offered for these files without taking
# over the user's default apps. UTImportedTypeDeclarations describe the
# types macOS itself may not know (they come with Microsoft Office or
# LibreOffice when those are installed, and those declarations win).
set -euo pipefail
version="$1"

doc_type() { # name role uti...
  local name="$1" role="$2"; shift 2
  printf '    <dict>\n      <key>CFBundleTypeName</key><string>%s</string>\n      <key>CFBundleTypeRole</key><string>%s</string>\n      <key>LSHandlerRank</key><string>Alternate</string>\n      <key>LSItemContentTypes</key><array>' "$name" "$role"
  for u in "$@"; do printf '<string>%s</string>' "$u"; done
  printf '</array>\n    </dict>\n'
}

imported() { # uti description conforms-to mime ext...
  local uti="$1" desc="$2" conforms="$3" mime="$4"; shift 4
  printf '    <dict>\n      <key>UTTypeIdentifier</key><string>%s</string>\n      <key>UTTypeDescription</key><string>%s</string>\n      <key>UTTypeConformsTo</key><array><string>%s</string></array>\n      <key>UTTypeTagSpecification</key><dict>\n        <key>public.filename-extension</key><array>' "$uti" "$desc" "$conforms"
  for e in "$@"; do printf '<string>%s</string>' "$e"; done
  printf '</array>\n        <key>public.mime-type</key><array><string>%s</string></array>\n      </dict>\n    </dict>\n' "$mime"
}

cat <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Kantor</string>
  <key>CFBundleDisplayName</key><string>Kantor</string>
  <key>CFBundleIdentifier</key><string>com.dzulfikar.kantor</string>
  <key>CFBundleExecutable</key><string>kantor</string>
  <key>CFBundleIconFile</key><string>kantor</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSHumanReadableCopyright</key><string>AGPL-3.0-or-later</string>
  <key>CFBundleDocumentTypes</key>
  <array>
$(doc_type "Spreadsheet" Editor org.openxmlformats.spreadsheetml.sheet org.openxmlformats.spreadsheetml.sheet.macroenabled public.comma-separated-values-text public.tab-separated-values-text)
$(doc_type "Spreadsheet (import)" Viewer com.microsoft.excel.xls com.microsoft.excel.xlt com.microsoft.excel.sheet.binary.macroenabled org.oasis-open.opendocument.spreadsheet)
$(doc_type "Document" Editor org.openxmlformats.wordprocessingml.document)
$(doc_type "Document (import)" Viewer public.rtf com.microsoft.word.doc com.microsoft.word.dot)
$(doc_type "Presentation" Editor org.openxmlformats.presentationml.presentation)
$(doc_type "Presentation (import)" Viewer com.microsoft.powerpoint.ppt com.microsoft.powerpoint.pps com.microsoft.powerpoint.pot)
$(doc_type "PDF document" Viewer com.adobe.pdf)
  </array>
  <key>UTImportedTypeDeclarations</key>
  <array>
$(imported org.openxmlformats.spreadsheetml.sheet.macroenabled "Excel Macro-Enabled Workbook" org.openxmlformats.openxml application/vnd.ms-excel.sheet.macroEnabled.12 xlsm)
$(imported com.microsoft.excel.xlt "Excel 97-2003 Template" public.data application/vnd.ms-excel xlt)
$(imported com.microsoft.excel.sheet.binary.macroenabled "Excel Binary Workbook" public.data application/vnd.ms-excel.sheet.binary.macroEnabled.12 xlsb)
$(imported org.oasis-open.opendocument.spreadsheet "OpenDocument Spreadsheet" public.zip-archive application/vnd.oasis.opendocument.spreadsheet ods)
$(imported public.tab-separated-values-text "Tab-Separated Values" public.delimited-values-text text/tab-separated-values tsv)
$(imported com.microsoft.word.dot "Word 97-2003 Template" public.data application/msword dot)
$(imported com.microsoft.powerpoint.pps "PowerPoint 97-2003 Slide Show" public.data application/vnd.ms-powerpoint pps)
$(imported com.microsoft.powerpoint.pot "PowerPoint 97-2003 Template" public.data application/vnd.ms-powerpoint pot)
  </array>
</dict>
</plist>
PLIST
