using OrbitDrop.Conversion;
using OrbitDrop.Conversion.Pdf;
using OrbitDrop.Core;
using PdfSharp.Drawing;
using PdfSharp.Fonts;
using PdfSharp.Pdf;
using PdfSharp.Pdf.IO;

/// <summary>
/// Onyx's own PDF tools, beside OrbitDrop's merge/split/rotate/compress: a text watermark, and adding or removing a
/// password. Text and passwords are asked for in a small window when the tool runs. Every tool writes a
/// new file next to the original ("report (watermarked).pdf"); originals are never changed.
/// </summary>
internal sealed class OnyxPdfTools : IOperationProvider
{
    public const string ProviderId = "onyx.pdf";
    public string Id => ProviderId;

    static OnyxPdfTools() => GlobalFontSettings.FontResolver ??= new SegoeFonts();

    /// <summary>
    /// Segoe UI, regular and bold, from the Windows fonts folder. PDFsharp's own Windows lookup knows only a few
    /// classic families, and its default resolver finds none.
    /// </summary>
    private sealed class SegoeFonts : IFontResolver
    {
        public FontResolverInfo ResolveTypeface(string familyName, bool bold, bool italic) =>
            new(bold ? "segoeuib" : "segoeui");

        public byte[]? GetFont(string faceName)
        {
            string path = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.Fonts), faceName + ".ttf");
            return File.Exists(path) ? File.ReadAllBytes(path) : null;
        }
    }

    public IEnumerable<OperationDescriptor> GetOperations(FileSet files, OperationMode mode)
    {
        if (mode != OperationMode.Tools || files.IsEmpty || !files.All(PdfProvider.IsPdf)) yield break;
        bool anyLocked = files.Any(IsLocked);
        if (!anyLocked)
        {
            yield return new("onyx.pdf.watermark", ProviderId, OperationMode.Tools, "Watermark", null, "", "", "Adding a watermark", 210);
            yield return new("onyx.pdf.protect", ProviderId, OperationMode.Tools, "Protect", null, "", "", "Adding a password", 220);
        }
        else
        {
            yield return new("onyx.pdf.unlock", ProviderId, OperationMode.Tools, "Unlock", null, "", "", "Removing the password", 220);
        }
    }

    public Task ExecuteAsync(OperationDescriptor operation, FileSet files, OperationContext context,
        IProgress<FileOutcome> progress, CancellationToken ct) => StaWorker.RunAsync(() =>
    {
        // Ask once for the whole batch, before any file is written.
        string? input = operation.Id switch
        {
            "onyx.pdf.watermark" => InputWindow.Ask("Watermark", "Text to place across every page", secret: false, confirm: false),
            "onyx.pdf.protect" => InputWindow.Ask("Protect PDF", "Password needed to open the PDF", secret: true, confirm: true),
            "onyx.pdf.unlock" => InputWindow.Ask("Unlock PDF", "The PDF's current password", secret: true, confirm: false),
            _ => "",
        };
        if (input is null)
        {
            foreach (var file in files) progress.Report(new FileOutcome(file, OutcomeStatus.Canceled, Message: "Canceled"));
            return;
        }

        foreach (var file in files)
        {
            ct.ThrowIfCancellationRequested();
            try
            {
                progress.Report(operation.Id switch
                {
                    "onyx.pdf.watermark" => Edit(file, context, ct, "watermarked", doc => Watermark(doc, input, ct)),
                    "onyx.pdf.protect" => Edit(file, context, ct, "protected", doc => Protect(doc, input)),
                    "onyx.pdf.unlock" => Unlock(file, input, context, ct),
                    _ => throw new InvalidOperationException(),
                });
            }
            catch (OperationCanceledException) { throw; }
            catch (Exception ex) { progress.Report(new FileOutcome(file, OutcomeStatus.Failed, Message: Friendly(ex))); }
        }
    }, "Onyx PDF tools");

    // ------------------------------------------------------------------ operations

    private static FileOutcome Edit(string file, OperationContext context, CancellationToken ct, string suffix, Action<PdfDocument> change)
    {
        using var doc = Open(file, null);
        change(doc);
        string output = SafeOutput.Write(file, context.Output, null, suffix, s => doc.Save(s, closeStream: false), PdfProvider.ValidatePdf, ct);
        return new(file, OutcomeStatus.Succeeded, output);
    }

    /// <summary>The text, large and faint, diagonally across the middle of every page.</summary>
    private static void Watermark(PdfDocument doc, string text, CancellationToken ct)
    {
        var brush = new XSolidBrush(XColor.FromArgb(56, 120, 120, 120));
        foreach (var page in doc.Pages)
        {
            ct.ThrowIfCancellationRequested();
            using var g = Upright(page, out double width, out double height);
            // Sized so the text spans 70% of the diagonal, whatever its length.
            double diagonal = Math.Sqrt(width * width + height * height);
            double measured = g.MeasureString(text, new XFont("Segoe UI", 100, XFontStyleEx.Bold)).Width;
            var font = new XFont("Segoe UI", Math.Clamp(100 * diagonal * 0.7 / Math.Max(1, measured), 12, 140), XFontStyleEx.Bold);
            g.TranslateTransform(width / 2, height / 2);
            g.RotateTransform(-Math.Atan2(height, width) * 180 / Math.PI);
            g.DrawString(text, font, brush, new XPoint(0, 0), XStringFormats.Center);
        }
    }

    /// <summary>AES-256, with the same password to open the PDF and to change it.</summary>
    private static void Protect(PdfDocument doc, string password)
    {
        doc.SecurityHandler.SetEncryptionToV5();
        doc.SecuritySettings.UserPassword = password;
        doc.SecuritySettings.OwnerPassword = password;
    }

    private static FileOutcome Unlock(string file, string password, OperationContext context, CancellationToken ct)
    {
        using var doc = Open(file, password);
        doc.SecurityHandler.SetEncryptionToNoneAndResetPasswords();
        string output = SafeOutput.Write(file, context.Output, null, "unlocked", s => doc.Save(s, closeStream: false), PdfProvider.ValidatePdf, ct);
        return new(file, OutcomeStatus.Succeeded, output);
    }

    // ------------------------------------------------------------------ helpers

    /// <summary>
    /// Graphics for drawing over a page in the coordinates a reader sees: (0, 0) at the visible top left, whatever the
    /// page's /Rotate. <paramref name="width"/> and <paramref name="height"/> are the visible size.
    /// </summary>
    private static XGraphics Upright(PdfPage page, out double width, out double height)
    {
        int rotate = ((page.Rotate % 360) + 360) % 360;
        // PDFsharp lays out XGraphics on the unrotated media box; undo the viewer's rotation so text reads upright.
        var g = XGraphics.FromPdfPage(page, XGraphicsPdfPageOptions.Append);
        double w = page.Width.Point, h = page.Height.Point;
        bool sideways = rotate is 90 or 270;
        width = sideways ? h : w;
        height = sideways ? w : h;
        switch (rotate)
        {
            // Visible (vx, vy) on a page shown 90° clockwise is (vy, h - vx) on the page itself.
            case 90: g.TranslateTransform(0, h); g.RotateTransform(270); break;
            case 180: g.TranslateTransform(w, h); g.RotateTransform(180); break;
            case 270: g.TranslateTransform(w, 0); g.RotateTransform(90); break;
        }
        return g;
    }

    private static bool IsLocked(string file)
    {
        try { using var _ = PdfReader.Open(file, PdfDocumentOpenMode.Import); return false; }
        catch (PdfReaderException ex) when (ex.Message.Contains("password", StringComparison.OrdinalIgnoreCase)) { return true; }
        catch { return false; }
    }

    private static PdfDocument Open(string file, string? password)
    {
        try
        {
            return password is null
                ? PdfReader.Open(file, PdfDocumentOpenMode.Modify)
                : PdfReader.Open(file, password, PdfDocumentOpenMode.Modify);
        }
        catch (PdfReaderException ex) when (ex.Message.Contains("password", StringComparison.OrdinalIgnoreCase))
        {
            throw new InvalidDataException(password is null ? "This PDF is password-protected." : "That password didn't open this PDF.");
        }
    }

    private static string Friendly(Exception ex) => ex switch
    {
        InvalidDataException => ex.Message,
        FileNotFoundException => "The file no longer exists.",
        UnauthorizedAccessException => "Windows denied access (read-only folder or file in use).",
        PdfReaderException => "This PDF couldn't be read (it may be damaged).",
        _ => "This PDF couldn't be changed.",
    };
}
