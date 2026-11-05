using System.Text.Json;
using OrbitDrop.Core;
using OrbitDrop.Conversion.Imaging;
using OrbitDrop.Conversion.Media;
using OrbitDrop.Conversion.Pdf;

// One short-lived process per catalog/conversion request. Standard output is
// private JSON IPC, never a transcript/file-name log or a background service.
internal static class Program
{
    private static readonly object OutputLock = new();
    private static readonly JsonSerializerOptions Json = new() { PropertyNameCaseInsensitive = true };
    private sealed record Request(string Command, string[] Files, string? Operation);

    [STAThread]
    private static int Main()
    {
        try { return Run().GetAwaiter().GetResult(); }
        catch { Send(new { type = "error", message = "The converter could not complete this request." }); return 1; }
    }

    private static async Task<int> Run()
    {
        string? line = await Console.In.ReadLineAsync();
        if (line is null || line.Length > 1_048_576) return 1;
        line = line.TrimStart('﻿'); // some callers' stdin writers start with a UTF-8 byte-order mark
        var request = JsonSerializer.Deserialize<Request>(line, Json);
        if (request is null || request.Files.Length is 0 or > 64) return 1;
        var files = new FileSet(request.Files);
        if (files.Any(f => !File.Exists(f)))
        {
            Send(new { type = "error", message = "Choose files stored on this PC. A selected file is missing or is a folder." });
            return 1;
        }
        var registry = new OperationRegistry([
            new ImageConversionProvider(), new PdfProvider(), new PdfToolsProvider(), new OnyxPdfTools(),
            new MediaFoundationProvider(), new MediaEditingProvider()
        ]);
        var options = PdfMenu.Arrange(
            registry.Query(files, OperationMode.Convert).Where(o => !o.NeedsInput),
            registry.Query(files, OperationMode.Tools).Where(o => !o.NeedsInput));
        if (request.Command == "catalog")
        {
            Send(new { type = "catalog", operations = options.Select(o => new { id = o.Id, label = o.Label }), preview = files.Count == 1 ? Preview.GetThumbnail(request.Files[0]) : null });
            return 0;
        }
        var operation = options.FirstOrDefault(o => o.Id == request.Operation);
        if (request.Command != "convert" || operation is null)
        {
            Send(new { type = "error", message = "That conversion is not available for these files on this PC." });
            return 1;
        }
        using var cancel = new CancellationTokenSource();
        _ = Task.Run(async () => {
            while (await Console.In.ReadLineAsync() is { } command)
                if (command == "cancel") { cancel.Cancel(); break; }
        });
        int completed = 0, succeeded = 0, failed = 0;
        var progress = new InlineProgress<FileOutcome>(outcome => {
            completed++;
            if (outcome.Status == OutcomeStatus.Succeeded) succeeded++;
            else if (outcome.Status == OutcomeStatus.Failed) failed++;
            Send(new { type = "outcome", succeeded = outcome.Status == OutcomeStatus.Succeeded,
                output = outcome.Output, message = outcome.Status == OutcomeStatus.Succeeded ? null : outcome.Message });
            Send(new { type = "progress", fraction = (double)completed / files.Count });
        });
        var context = new OperationContext {
            Output = new OutputOptions(), Shell = new NoShell(), JpegQuality = 92,
            ReportFileFraction = fraction => Send(new { type = "progress", fraction = Math.Clamp((completed + fraction) / files.Count, 0, 1) })
        };
        try {
            await registry.ProviderFor(operation).ExecuteAsync(operation, files, context, progress, cancel.Token);
            Send(new { type = "complete", succeeded, failed, canceled = false });
        } catch (OperationCanceledException) {
            Send(new { type = "complete", succeeded, failed, canceled = true });
        }
        return 0;
    }

    private static void Send(object value)
    {
        lock (OutputLock) { Console.Out.WriteLine(JsonSerializer.Serialize(value)); Console.Out.Flush(); }
    }

    private sealed class InlineProgress<T>(Action<T> report) : IProgress<T>
    {
        public void Report(T value) => report(value);
    }

    private sealed class NoShell : IShellIntegration
    {
        public void SetClipboardText(string text) => throw new NotSupportedException();
        public void Reveal(IReadOnlyList<string> paths) => throw new NotSupportedException();
    }
}
