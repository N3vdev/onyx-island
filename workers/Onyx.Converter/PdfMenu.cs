using OrbitDrop.Core;

/// <summary>
/// The island's choices for the dropped files: PDF tools first (most used first, short labels), then the format
/// conversions. The island card shows at most <see cref="MaxOptions"/> buttons. Other tools stay out of the island.
/// </summary>
internal static class PdfMenu
{
    public const int MaxOptions = 8;

    /// <summary>Tools offered in the island, in this order, with these labels.</summary>
    private static readonly (string Id, string Label)[] Tools =
    [
        ("tool.pdf.merge", "Merge"),
        ("tool.image.pdf-combine", "One PDF"),
        ("onyx.pdf.unlock", "Unlock"),
        ("tool.pdf.compress", "Compress"),
        ("tool.pdf.split", "Split"),
        ("tool.pdf.rotate.right", "Rotate"),
        ("onyx.pdf.watermark", "Watermark"),
        ("onyx.pdf.protect", "Protect"),
    ];

    public static OperationDescriptor[] Arrange(IEnumerable<OperationDescriptor> conversions, IEnumerable<OperationDescriptor> tools)
    {
        var offered = tools.Where(t => !t.IsGroupHeader).ToDictionary(t => t.Id);
        var picked = Tools.Where(t => offered.ContainsKey(t.Id)).Select(t => offered[t.Id] with { Label = t.Label }).ToList();
        // A password-locked PDF can't be opened by any other tool or conversion: offer only Unlock.
        if (picked.FirstOrDefault(t => t.Id == "onyx.pdf.unlock") is { } unlock) return [unlock];
        var formats = conversions.Where(c => !c.IsGroupHeader).ToList();
        if (picked.Count == 0) return formats.ToArray();
        return picked.Concat(formats).Take(MaxOptions).ToArray();
    }
}
