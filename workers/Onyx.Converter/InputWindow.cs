using System.Windows;
using System.Windows.Controls;
using System.Windows.Input;
using System.Windows.Media;

/// <summary>
/// The one question a PDF tool needs (watermark text or a password), in a small dark window centred on the screen.
/// Enter confirms, Esc cancels. Must be called on an STA thread. Nothing typed here is logged or stored.
/// </summary>
internal static class InputWindow
{
    private static readonly Brush Ground = new SolidColorBrush(Color.FromRgb(0x16, 0x16, 0x18));
    private static readonly Brush Field = new SolidColorBrush(Color.FromRgb(0x24, 0x24, 0x27));
    private static readonly Brush Text = new SolidColorBrush(Color.FromRgb(0xF2, 0xF2, 0xF4));
    private static readonly Brush Muted = new SolidColorBrush(Color.FromRgb(0xA0, 0xA0, 0xA8));
    private static readonly Brush Error = new SolidColorBrush(Color.FromRgb(0xF2, 0x8B, 0x82));

    /// <summary>The value typed, or null if the window was canceled.</summary>
    public static string? Ask(string title, string prompt, bool secret, bool confirm)
    {
        string? result = null;
        var window = new Window
        {
            Title = $"Onyx · {title}", Width = 380, SizeToContent = SizeToContent.Height, ResizeMode = ResizeMode.NoResize,
            WindowStartupLocation = WindowStartupLocation.CenterScreen, Topmost = true, Background = Ground,
            ShowInTaskbar = true,
        };
        var panel = new StackPanel { Margin = new Thickness(22, 20, 22, 20) };
        panel.Children.Add(new TextBlock { Text = title, Foreground = Text, FontSize = 17, FontWeight = FontWeights.SemiBold });
        panel.Children.Add(new TextBlock { Text = prompt, Foreground = Muted, FontSize = 12.5, Margin = new Thickness(0, 4, 0, 14), TextWrapping = TextWrapping.Wrap });

        Control first = secret ? Styled(new PasswordBox()) : Styled(new TextBox());
        Control? second = secret && confirm ? Styled(new PasswordBox()) : null;
        panel.Children.Add(first);
        if (second is not null)
        {
            panel.Children.Add(new TextBlock { Text = "Type it again", Foreground = Muted, FontSize = 12.5, Margin = new Thickness(0, 10, 0, 6) });
            panel.Children.Add(second);
        }
        var message = new TextBlock { Foreground = Error, FontSize = 12, Margin = new Thickness(0, 8, 0, 0), Visibility = Visibility.Collapsed };
        panel.Children.Add(message);

        var buttons = new StackPanel { Orientation = Orientation.Horizontal, HorizontalAlignment = HorizontalAlignment.Right, Margin = new Thickness(0, 16, 0, 0) };
        var cancel = Button("Cancel", primary: false);
        var ok = Button(secret && confirm ? "Protect" : "OK", primary: true);
        cancel.IsCancel = true;
        ok.IsDefault = true;
        buttons.Children.Add(cancel);
        buttons.Children.Add(ok);
        panel.Children.Add(buttons);
        window.Content = panel;

        string Value(Control c) => c is PasswordBox p ? p.Password : ((TextBox)c).Text.Trim();
        ok.Click += (_, _) =>
        {
            string value = Value(first);
            string? problem = value.Length == 0 ? "Type something first."
                : value.Length > 200 ? "That's too long."
                : second is not null && Value(second) != value ? "The two passwords don't match."
                : null;
            if (problem is not null)
            {
                message.Text = problem;
                message.Visibility = Visibility.Visible;
                return;
            }
            result = value;
            window.DialogResult = true;
        };
        window.Loaded += (_, _) => { window.Activate(); first.Focus(); Keyboard.Focus(first); };
        window.ShowDialog();
        return result;
    }

    private static Control Styled(Control box)
    {
        box.Background = Field;
        box.Foreground = Text;
        box.BorderBrush = new SolidColorBrush(Color.FromRgb(0x3A, 0x3A, 0x40));
        box.Padding = new Thickness(8, 6, 8, 6);
        box.FontSize = 14;
        if (box is TextBox t) t.CaretBrush = Text;
        if (box is PasswordBox p) p.CaretBrush = Text;
        return box;
    }

    private static Button Button(string label, bool primary) => new()
    {
        Content = label, MinWidth = 84, Padding = new Thickness(14, 6, 14, 6), Margin = new Thickness(8, 0, 0, 0),
        Background = primary ? Text : Field, Foreground = primary ? Ground : Text, BorderThickness = new Thickness(0),
        FontSize = 13, Cursor = Cursors.Hand,
    };
}
