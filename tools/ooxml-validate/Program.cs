// Validates .xlsx/.docx/.pptx files with the Open XML SDK (the schema and
// semantic checks Office runs before it offers "repair") and prints one JSON
// object per file on stdout:
//   {"file": "...", "open_error": null, "errors": [{"part", "id", "type", "description", "path"}]}
// Usage: OoxmlValidate <file>...   (exit code 0 even when files have errors;
// tools/fidelity/office_check.py decides what is new).
using System.Text.Json;
using DocumentFormat.OpenXml;
using DocumentFormat.OpenXml.Packaging;
using DocumentFormat.OpenXml.Validation;

var validator = new OpenXmlValidator(FileFormatVersions.Microsoft365) { MaxNumberOfErrors = 0 };
var json = new JsonSerializerOptions { WriteIndented = false };
foreach (var file in args)
{
    var errors = new List<object>();
    string? openError = null;
    try
    {
        var settings = new OpenSettings { AutoSave = false };
        using OpenXmlPackage doc = Path.GetExtension(file).ToLowerInvariant() switch
        {
            ".xlsx" or ".xlsm" => SpreadsheetDocument.Open(file, false, settings),
            ".docx" or ".docm" => WordprocessingDocument.Open(file, false, settings),
            ".pptx" or ".pptm" => PresentationDocument.Open(file, false, settings),
            var e => throw new NotSupportedException($"unknown extension {e}"),
        };
        foreach (var e in validator.Validate(doc))
        {
            errors.Add(new
            {
                part = e.Part?.Uri.ToString(),
                id = e.Id,
                type = e.ErrorType.ToString(),
                description = e.Description,
                path = e.Path?.XPath,
                node = e.Node?.LocalName,
            });
        }
    }
    catch (Exception ex)
    {
        openError = $"{ex.GetType().Name}: {ex.Message}";
    }
    Console.WriteLine(JsonSerializer.Serialize(new { file, open_error = openError, errors }, json));
}
