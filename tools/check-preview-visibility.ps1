# Live native regression check. Run with Onyx and Onyx Settings open.
# Leaves Settings minimized and the island on the desktop.
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class OnyxPreviewLifecycleCheck {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr FindWindow(string c, string t);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern IntPtr FindWindowEx(IntPtr p, IntPtr a, string c, string t);
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr GetProp(IntPtr h, string n);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int s);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
    [DllImport("user32.dll")] public static extern IntPtr GetWindowLongPtr(IntPtr h, int i);
    public static IntPtr Settings() { return FindWindow(null, "Onyx Settings"); }
    public static IntPtr Island() { return FindWindow("IsletIslandWindow", null); }
    public static IntPtr Anchor(IntPtr p) { return FindWindowEx(p, IntPtr.Zero, null, "IsletPreviewAnchor"); }
}
'@
$settingsRoot = [OnyxPreviewLifecycleCheck]::Settings()
$islandRoot = [OnyxPreviewLifecycleCheck]::Island()
$previewAnchor = [OnyxPreviewLifecycleCheck]::Anchor($settingsRoot)
if ($settingsRoot -eq [IntPtr]::Zero -or $islandRoot -eq [IntPtr]::Zero -or $previewAnchor -eq [IntPtr]::Zero) {
    throw 'Open Onyx and Onyx Settings before running this check.'
}
function Wait-PreviewState([long]$expected) {
    for ($attempt = 0; $attempt -lt 70; $attempt++) {
        if ([OnyxPreviewLifecycleCheck]::GetProp($previewAnchor, 'IsletPreviewAttached').ToInt64() -eq $expected -and
            [OnyxPreviewLifecycleCheck]::IsWindowVisible($islandRoot)) { return }
        Start-Sleep -Milliseconds 100
    }
    throw "Preview did not settle at state $expected."
}
function Assert-DesktopIsland {
    Wait-PreviewState 0
    if (([OnyxPreviewLifecycleCheck]::GetWindowLongPtr($islandRoot, -20).ToInt64() -band 8) -eq 0) {
        throw 'Desktop island did not regain its topmost state.'
    }
}
[void][OnyxPreviewLifecycleCheck]::ShowWindow($settingsRoot, 9)
Wait-PreviewState 2
Write-Output 'PASS visible settings attaches preview.'
[void][OnyxPreviewLifecycleCheck]::ShowWindow($settingsRoot, 6)
Assert-DesktopIsland
if (![OnyxPreviewLifecycleCheck]::IsIconic($settingsRoot)) { throw 'Settings was not minimized.' }
Write-Output 'PASS minimizing returns a visible, topmost desktop island.'
[void][OnyxPreviewLifecycleCheck]::PostMessage($islandRoot, 0x8028, $previewAnchor, [IntPtr]::Zero)
Start-Sleep -Milliseconds 400
Assert-DesktopIsland
Write-Output 'PASS stale attach while minimized is ignored.'
[void][OnyxPreviewLifecycleCheck]::ShowWindow($settingsRoot, 9)
Wait-PreviewState 2
Write-Output 'PASS restoring reattaches preview.'
[void][OnyxPreviewLifecycleCheck]::ShowWindow($settingsRoot, 6)
Start-Sleep -Milliseconds 120
[void][OnyxPreviewLifecycleCheck]::ShowWindow($settingsRoot, 9)
Start-Sleep -Milliseconds 1600
Wait-PreviewState 2
Write-Output 'PASS rapid minimize/restore settles in preview.'
[void][OnyxPreviewLifecycleCheck]::ShowWindow($settingsRoot, 0)
Assert-DesktopIsland
Write-Output 'PASS hidden settings releases the desktop island.'
[void][OnyxPreviewLifecycleCheck]::ShowWindow($settingsRoot, 9)
Wait-PreviewState 2
Write-Output 'PASS showing settings reattaches preview.'
[void][OnyxPreviewLifecycleCheck]::ShowWindow($settingsRoot, 6)
Assert-DesktopIsland
[void][OnyxPreviewLifecycleCheck]::ShowWindow($settingsRoot, 9)
$caughtPending = $false
for ($attempt = 0; $attempt -lt 200; $attempt++) {
    if ([OnyxPreviewLifecycleCheck]::GetProp($previewAnchor, 'IsletPreviewAttached').ToInt64() -eq 1) {
        $caughtPending = $true
        break
    }
    Start-Sleep -Milliseconds 10
}
if (!$caughtPending) { throw 'Could not exercise the pending-attach race.' }
[void][OnyxPreviewLifecycleCheck]::ShowWindow($settingsRoot, 6)
Assert-DesktopIsland
Start-Sleep -Milliseconds 1100
Assert-DesktopIsland
Write-Output 'PASS minimizing during attachment clears the pending state.'
[void][OnyxPreviewLifecycleCheck]::ShowWindow($settingsRoot, 9)
Wait-PreviewState 2
[void][OnyxPreviewLifecycleCheck]::ShowWindow($settingsRoot, 6)
Assert-DesktopIsland
Write-Output 'PASS final state: settings minimized, desktop island visible.'
