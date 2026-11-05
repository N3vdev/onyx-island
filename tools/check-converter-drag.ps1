# Real cross-process OLE drag of an authored PDF into the running island.
# Captures only the island rectangle. Never opens or logs user documents.
param([string]$FixturePath,[switch]$DropThenChoose,[string]$CapturePrefix='converter',[switch]$CaptureOnly,[switch]$InternalWorker)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
if(-not $InternalWorker -and -not $CaptureOnly){
    if(-not $FixturePath){throw 'FixturePath is required for a drag check.'}
    $runId=[Guid]::NewGuid().ToString('N').Substring(0,8)
    $stdout=Join-Path $root "target\converter-native-$runId.out"
    $stderr=Join-Path $root "target\converter-native-$runId.err"
    $arguments=@('-NoProfile','-STA','-File',('"'+$PSCommandPath+'"'),'-InternalWorker','-FixturePath',('"'+$FixturePath+'"'),'-CapturePrefix',$CapturePrefix)
    if($DropThenChoose){$arguments+='-DropThenChoose'}
    $child=Start-Process -FilePath 'powershell.exe' -WindowStyle Hidden -ArgumentList $arguments -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
    if(-not $child.WaitForExit(20000)){
        Stop-Process -Id $child.Id -ErrorAction SilentlyContinue
        throw 'The interactive desktop check timed out; the owned test process was stopped.'
    }
    Get-Content -LiteralPath $stdout
    if($child.ExitCode -ne 0){Get-Content -LiteralPath $stderr;throw 'Native interaction check failed.'}
    return
}
$review = Join-Path $root '.impeccable\review'
New-Item -ItemType Directory -Path $review -Force | Out-Null
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Windows.Forms,System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Runtime.InteropServices.ComTypes;
using System.Threading;
using System.Windows.Forms;

public static class OnyxDragCheck {
    static volatile bool CancelRequested;
    [StructLayout(LayoutKind.Sequential)] struct POINT { public int X,Y; }
    [StructLayout(LayoutKind.Sequential)] struct RECT { public int L,T,R,B; }
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern IntPtr FindWindow(string cls,string name);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h,out RECT r);
    [DllImport("user32.dll")] static extern int GetWindowRgn(IntPtr h,IntPtr r);
    [DllImport("gdi32.dll")] static extern IntPtr CreateRectRgn(int l,int t,int r,int b);
    [DllImport("gdi32.dll")] static extern int GetRgnBox(IntPtr h,out RECT r);
    [DllImport("gdi32.dll")] static extern bool DeleteObject(IntPtr h);
    [DllImport("user32.dll")] static extern bool SetCursorPos(int x,int y);
    [DllImport("user32.dll")] static extern bool GetCursorPos(out POINT p);
    [DllImport("user32.dll")] static extern void mouse_event(uint flags,uint x,uint y,uint data,UIntPtr extra);
    [DllImport("user32.dll")] static extern IntPtr SendMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
    [DllImport("user32.dll")] static extern IntPtr SetThreadDpiAwarenessContext(IntPtr value);
    static Rectangle Frame(IntPtr hwnd) {
        RECT window,region; GetWindowRect(hwnd,out window);
        IntPtr r=CreateRectRgn(0,0,0,0);
        try { GetWindowRgn(hwnd,r); GetRgnBox(r,out region); }
        finally {DeleteObject(r);}
        return Rectangle.FromLTRB(window.L+region.L,window.T+region.T,window.L+region.R,window.T+region.B);
    }
    public static void Capture(string path) {
        SetThreadDpiAwarenessContext(new IntPtr(-4));
        IntPtr hwnd=FindWindow("IsletIslandWindow",null);
        Rectangle r=Frame(hwnd);r.Inflate(6,6);
        if(r.Width<20||r.Height<20||r.Width>900||r.Height>500)throw new Exception("Unexpected island capture bounds");
        using(var bitmap=new Bitmap(r.Width,r.Height,PixelFormat.Format24bppRgb)) {
            using(var graphics=Graphics.FromImage(bitmap))graphics.CopyFromScreen(r.Location,Point.Empty,r.Size);
            bitmap.Save(path,ImageFormat.Png);
        }
    }
    public static string Run(string file,string capture,bool dropBody) {
        CancelRequested=false;
        POINT original;GetCursorPos(out original);
        if(Thread.CurrentThread.GetApartmentState()!=ApartmentState.STA)throw new Exception("Run this check with powershell.exe -STA");
        var timeout=new System.Threading.Timer(_=>{CancelRequested=true;mouse_event(4,0,0,0,UIntPtr.Zero);SetCursorPos(original.X,original.Y);},null,12000,Timeout.Infinite);
        try {
            return RunSta(file,capture,dropBody);
        }finally{timeout.Dispose();CancelRequested=true;mouse_event(4,0,0,0,UIntPtr.Zero);SetCursorPos(original.X,original.Y);}
    }
    static string RunSta(string file,string capture,bool dropBody) {
        SetThreadDpiAwarenessContext(new IntPtr(-4));
        IntPtr hwnd=FindWindow("IsletIslandWindow",null);
        if(hwnd==IntPtr.Zero)throw new Exception("Onyx is not running");
        Exception moveError=null;DragDropEffects result=DragDropEffects.None;bool started=false,release=false;
        using(var form=new Form()) {
            form.Text="Onyx converter test";form.ShowInTaskbar=false;form.TopMost=true;
            form.StartPosition=FormStartPosition.Manual;form.Location=new Point(25,300);form.ClientSize=new Size(200,60);
            var label=new Label{Text="Authored PDF drag fixture",Dock=DockStyle.Fill,TextAlign=ContentAlignment.MiddleCenter};
            form.Controls.Add(label);
            Action beginDrag=()=>{
                if(started)return;
                started=true;
                var data=new System.Windows.Forms.DataObject(DataFormats.FileDrop,new string[]{file});
                result=label.DoDragDrop(data,DragDropEffects.Copy);
                form.BeginInvoke(new Action(()=>form.Close()));
            };
            label.MouseDown+=(sender,args)=>{if(args.Button==MouseButtons.Left)beginDrag();};
            label.QueryContinueDrag+=(sender,args)=>{args.Action=CancelRequested?DragAction.Cancel:release?DragAction.Drop:DragAction.Continue;};
            form.Shown+=(sender,args)=>{
                var mover=new Thread(()=>{
                    try {
                        SetThreadDpiAwarenessContext(new IntPtr(-4));
                        Thread.Sleep(250);Point start=label.PointToScreen(new Point(90,30));
                        SetCursorPos(start.X,start.Y);mouse_event(2,0,0,0,UIntPtr.Zero);
                        form.BeginInvoke(beginDrag);
                        for(int attempt=0;attempt<20&&!started;attempt++)Thread.Sleep(100);
                        Rectangle r=Frame(hwnd);
                        if(!started)throw new Exception("The authored source did not start a drag");
                        SetCursorPos(r.Left+r.Width/2,r.Top+r.Height/2);
                        Thread.Sleep(1500);r=Frame(hwnd);
                        double scale=r.Width/496.0;
                        if(r.Height<170*scale)throw new Exception("OLE drag did not open the converter (frame "+r.Width+"x"+r.Height+")");
                        int x=r.Left+(int)((dropBody?150:70)*scale),y=r.Top+(int)((dropBody?46:88)*scale);
                        SetCursorPos(x,y);Thread.Sleep(250);Capture(capture);
                        release=true;mouse_event(4,0,0,0,UIntPtr.Zero);SetCursorPos(x+1,y);
                    }catch(Exception e){moveError=e;CancelRequested=true;mouse_event(4,0,0,0,UIntPtr.Zero);form.BeginInvoke(new Action(()=>form.Close()));}
                });
                mover.IsBackground=true;mover.Start();
            };
            Application.Run(form);
        }
        if(moveError!=null)throw moveError;
        if(result!=DragDropEffects.Copy)throw new Exception("OLE drop did not finish as COPY");
        return "OLE drop: COPY";
    }
    public static void ChooseWithKeyboard(string capture) {
        IntPtr hwnd=FindWindow("IsletIslandWindow",null);Rectangle r=Frame(hwnd);
        POINT original;GetCursorPos(out original);double scale=r.Width/496.0;
        try {
            SetCursorPos(r.Left+(int)(150*scale),r.Top+(int)(46*scale));
            mouse_event(2,0,0,0,UIntPtr.Zero);Thread.Sleep(60);mouse_event(4,0,0,0,UIntPtr.Zero);
            Thread.Sleep(120);SendMessage(hwnd,0x100,new IntPtr(9),IntPtr.Zero);
            Thread.Sleep(100);Capture(capture);
            SendMessage(hwnd,0x100,new IntPtr(13),IntPtr.Zero);
        }finally{mouse_event(4,0,0,0,UIntPtr.Zero);SetCursorPos(original.X,original.Y);}
    }
}
'@
if($CaptureOnly){[OnyxDragCheck]::Capture((Join-Path $review ($CapturePrefix+'-complete.png')));return}
if(-not $FixturePath){throw 'FixturePath is required for a drag check.'}
$fixture = (Resolve-Path -LiteralPath $FixturePath).Path
$dragRoot = Join-Path (Split-Path -Parent $fixture) ('native-' + [Guid]::NewGuid().ToString('N').Substring(0,8))
New-Item -ItemType Directory -Path $dragRoot | Out-Null
$extension = [IO.Path]::GetExtension($fixture)
$native = Join-Path $dragRoot ('Native drag sample' + $extension)
Copy-Item -LiteralPath $fixture -Destination $native -Force
$before = (Get-FileHash -LiteralPath $native -Algorithm SHA256).Hash
[OnyxDragCheck]::Run($native,(Join-Path $review ($CapturePrefix+'-ready.png')),$DropThenChoose.IsPresent)
if ($DropThenChoose) { [OnyxDragCheck]::ChooseWithKeyboard((Join-Path $review ($CapturePrefix+'-keyboard.png'))) }
$deadline=(Get-Date).AddSeconds(10)
$output=if($extension -eq '.pdf'){Join-Path $dragRoot 'Native drag sample (PNG)'}else{Join-Path $dragRoot 'Native drag sample.png'}
while(-not (Test-Path -LiteralPath $output) -and (Get-Date) -lt $deadline){Start-Sleep -Milliseconds 100}
if(-not (Test-Path -LiteralPath $output)){throw 'Native drop did not produce its PNG folder.'}
if($extension -eq '.pdf' -and @(Get-ChildItem -LiteralPath $output -Filter '*.png').Count -ne 2){throw 'Expected two converted pages.'}
if((Get-FileHash -LiteralPath $native -Algorithm SHA256).Hash -ne $before){throw 'Original changed.'}
Start-Sleep -Milliseconds 500
[OnyxDragCheck]::Capture((Join-Path $review ($CapturePrefix+'-complete.png')))
'PASS real OLE drop, conversion, original retained.'
