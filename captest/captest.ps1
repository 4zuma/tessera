# Temporary probe: does the page's maximize button react to a real mouse?
param([string]$Exe, [string]$Out = 'shots', [string]$Tag = 'run')
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Out | Out-Null
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class W {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr c);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, int x, int y, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint m, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumChildWindows(IntPtr h, EnumProc p, IntPtr l);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern IntPtr GetParent(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc p, IntPtr l);
  public static IntPtr Biggest(uint pid) {
    IntPtr best = IntPtr.Zero; long area = 0;
    EnumProc p = (h, x) => {
      uint id; GetWindowThreadProcessId(h, out id);
      RECT r;
      if (id == pid && IsWindowVisible(h) && GetWindowRect(h, out r)) {
        long a = (long)(r.R - r.L) * (r.B - r.T);
        if (a > area) { area = a; best = h; }
      }
      return true;
    };
    EnumWindows(p, IntPtr.Zero);
    GC.KeepAlive(p);
    return area > 40000 ? best : IntPtr.Zero;
  }
  public static string Cls(IntPtr h) { var s = new StringBuilder(256); GetClassName(h, s, 256); return s.ToString(); }
  public static List<IntPtr> Children(IntPtr h) {
    var l = new List<IntPtr>();
    EnumProc p = (c, x) => { l.Add(c); return true; };
    EnumChildWindows(h, p, IntPtr.Zero);
    GC.KeepAlive(p);
    return l;
  }
  public static IntPtr LParam(int x, int y) { return (IntPtr)(((y & 0xffff) << 16) | (x & 0xffff)); }
}
"@
# per-monitor aware: every coordinate here is physical
[W]::SetThreadDpiAwarenessContext([IntPtr]-4) | Out-Null

function Shot($name) {
  $w = [W]::GetSystemMetrics(0); $h = [W]::GetSystemMetrics(1)
  $bmp = New-Object System.Drawing.Bitmap $w, $h
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen(0, 0, 0, 0, $bmp.Size)
  $bmp.Save((Join-Path $Out "$Tag-$name.png"))
  $g.Dispose(); $bmp.Dispose()
}
function Describe($h) {
  $r = New-Object W+RECT; [W]::GetWindowRect($h, [ref]$r) | Out-Null
  $procId = [uint32]0; $tid = [W]::GetWindowThreadProcessId($h, [ref]$procId)
  '{0:X8} {1,-34} parent {2:X8} pid {3,6} tid {4,6} vis {5} rect {6},{7} {8}x{9}' -f [int64]$h, [W]::Cls($h), [int64][W]::GetParent($h), $procId, $tid, [W]::IsWindowVisible($h), $r.L, $r.T, ($r.R - $r.L), ($r.B - $r.T)
}

Write-Host "screen $([W]::GetSystemMetrics(0))x$([W]::GetSystemMetrics(1))"
$p = Start-Process -FilePath $Exe -PassThru
$main = [IntPtr]::Zero
for ($i = 0; $i -lt 90; $i++) {
  Start-Sleep -Milliseconds 500
  $main = [W]::Biggest([uint32]$p.Id)
  if ($main -ne [IntPtr]::Zero) { break }
}
if ($main -eq [IntPtr]::Zero) { Shot 'nowindow'; throw 'no window' }
Start-Sleep -Seconds 6
[W]::SetForegroundWindow($main) | Out-Null
Start-Sleep -Milliseconds 500
Write-Host "main: $(Describe $main)"
Write-Host "app thread: $([W]::GetWindowThreadProcessId($main, [ref]([uint32]0)))"
foreach ($c in [W]::Children($main)) { Write-Host "  $(Describe $c)" }

$dpi = [W]::GetDpiForWindow($main); $k = $dpi / 96.0
$cr = New-Object W+RECT; [W]::GetClientRect($main, [ref]$cr) | Out-Null
Write-Host "dpi $dpi client $($cr.R)x$($cr.B)"

function Probe($label, $cssFromRight) {
  $pt = New-Object W+POINT
  $pt.X = [int]($cr.R - $cssFromRight * $k); $pt.Y = [int](20 * $k)
  [W]::ClientToScreen($main, [ref]$pt) | Out-Null
  $at = [W]::WindowFromPoint($pt)
  $ht = [int64][W]::SendMessage($main, 0x84, [IntPtr]::Zero, [W]::LParam($pt.X, $pt.Y))
  Write-Host "$label at $($pt.X),$($pt.Y): WindowFromPoint $(Describe $at); main WM_NCHITTEST $ht"
  foreach ($c in [W]::Children($main)) {
    if ([W]::GetParent($c) -eq $main) {
      $hc = [int64][W]::SendMessage($c, 0x84, [IntPtr]::Zero, [W]::LParam($pt.X, $pt.Y))
      Write-Host "    child $([W]::Cls($c)) WM_NCHITTEST $hc"
    }
  }
  return $pt
}

$min = Probe 'min' 115
$max = Probe 'max' 69
$cls = Probe 'close' 23

function MoveTo($pt) {
  [W]::SetCursorPos($pt.X - 3, $pt.Y) | Out-Null
  Start-Sleep -Milliseconds 100
  for ($i = 0; $i -lt 3; $i++) { [W]::mouse_event(1, 1, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 50 }
}
function Click() {
  [W]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 90
  [W]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
}

Shot '0-start'
MoveTo $min; Start-Sleep -Milliseconds 800; Shot '1-hover-min'
MoveTo $max; Start-Sleep -Milliseconds 800; Shot '2-hover-max'
Start-Sleep -Seconds 2; Shot '3-rest-max'
Write-Host "before click: zoomed $([W]::IsZoomed($main))"
Click; Start-Sleep -Seconds 2
$z1 = [W]::IsZoomed($main)
Write-Host "after click 1: zoomed $z1"
Shot '4-after-click'
if ($z1) {
  $cr = New-Object W+RECT; [W]::GetClientRect($main, [ref]$cr) | Out-Null
  $max = Probe 'max (maximized)' 69
  MoveTo $max; Start-Sleep -Milliseconds 800; Shot '5-hover-restore'
  Click; Start-Sleep -Seconds 2
  Write-Host "after click 2: zoomed $([W]::IsZoomed($main))"
  Shot '6-after-restore'
}
Stop-Process -Id $p.Id -Force
if (-not $z1) { Write-Host '::error::maximize button did not maximize' }
