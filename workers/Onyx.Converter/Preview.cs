using System.Runtime.InteropServices;
using System.Windows;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Media.Imaging;

internal static class Preview
{
    [StructLayout(LayoutKind.Sequential)] private struct Size { public int Width, Height; }
    [ComImport, Guid("bcc18b79-ba16-442f-80c4-8a59c30c463b"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IImageFactory { void GetImage(Size size, uint flags, out IntPtr bitmap); }
    [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
    private static extern void SHCreateItemFromParsingName(string path, IntPtr context, ref Guid iid, [MarshalAs(UnmanagedType.Interface)] out IImageFactory item);
    [DllImport("gdi32.dll")] private static extern bool DeleteObject(IntPtr obj);

    public static int[]? GetThumbnail(string path)
    {
        IImageFactory? factory = null;
        IntPtr bitmap = IntPtr.Zero;
        try {
            BitmapSource? source = null;
            // Decode images directly so a PNG preview does not depend on the
            // Explorer thumbnail cache or a registered Shell provider.
            if (new[] { ".png", ".jpg", ".jpeg", ".bmp", ".gif", ".tif", ".tiff" }.Contains(System.IO.Path.GetExtension(path).ToLowerInvariant())) {
                try {
                    var image = new BitmapImage();
                    image.BeginInit();
                    image.CacheOption = BitmapCacheOption.OnLoad;
                    image.DecodePixelWidth = 256;
                    image.UriSource = new Uri(System.IO.Path.GetFullPath(path));
                    image.EndInit();
                    image.Freeze();
                    source = image;
                } catch { /* Try the Shell provider next. */ }
            }
            if (source is null) {
                var iid = typeof(IImageFactory).GUID;
                SHCreateItemFromParsingName(path, IntPtr.Zero, ref iid, out factory);
                factory.GetImage(new Size { Width = 256, Height = 256 }, 0x08, out bitmap);
                source = Imaging.CreateBitmapSourceFromHBitmap(bitmap, IntPtr.Zero, Int32Rect.Empty, BitmapSizeOptions.FromEmptyOptions());
            }
            var visual = new DrawingVisual();
            using (var drawing = visual.RenderOpen()) {
                drawing.DrawRectangle(new SolidColorBrush(Color.FromRgb(28, 28, 30)), null, new Rect(0, 0, 256, 256));
                double scale = Math.Min(256.0 / source.PixelWidth, 256.0 / source.PixelHeight);
                double width = source.PixelWidth * scale, height = source.PixelHeight * scale;
                drawing.DrawImage(source, new Rect((256-width)/2, (256-height)/2, width, height));
            }
            var target = new RenderTargetBitmap(256, 256, 96, 96, PixelFormats.Pbgra32);
            target.Render(visual);
            var pixels = new byte[256 * 256 * 4];
            target.CopyPixels(pixels, 256 * 4, 0);
            return pixels.Select(b => (int)b).ToArray();
        } catch { return null; /* Keep the native file icon. */ }
        finally {
            if (bitmap != IntPtr.Zero) DeleteObject(bitmap);
            if (factory is not null) Marshal.ReleaseComObject(factory);
        }
    }
}
