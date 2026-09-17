# Capture the running Vitna Code window to a PNG, for design review.
#
#   scripts/capture-window.ps1 <vitna-code.exe> <workspace dir> <out.png> [keep]
#
# Launches the binary on the workspace, waits for its titled window, and
# renders THAT WINDOW ONLY through PrintWindow on its handle. It never reads
# the screen and never changes focus, so the picture cannot include another
# window's contents, whatever is in front. Without "keep" the process is
# stopped once the file is written; with it, the window is left running.
# Environment hooks the window honours for a capture: VITNA_GUI_OPEN_MENU=1,
# VITNA_GUI_OPEN_MODEL_MENU=1, VITNA_GUI_OPEN_SETTINGS=<general|shortcuts|
# daemon|providers|models>. Windows only; the MinGW runtime DLLs are put on
# PATH for a gnu-toolchain build. The first launch after a fresh build can
# photograph the window before it has sized; run it again.
#
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public class Win {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
$exe = $args[0]; $ws = $args[1]; $out = $args[2]
$env:PATH = "C:/msys64/mingw64/bin;" + $env:PATH
$p = Start-Process -FilePath $exe -ArgumentList $ws -PassThru
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 48; $i++) {
  Start-Sleep -Milliseconds 250; $p.Refresh()
  if ($p.HasExited) { "EXITED after {0}ms with code 0x{1:X8}" -f ($i*250), $p.ExitCode; exit 2 }
  if ($p.MainWindowHandle -ne 0 -and $p.MainWindowTitle -like "Vitna Code*") { $h = $p.MainWindowHandle; "window '{0}' after {1}ms" -f $p.MainWindowTitle, ($i*250); break }
}
if ($h -eq [IntPtr]::Zero) { "no titled window after 12s; alive=$(-not $p.HasExited)"; Stop-Process -Id $p.Id -ErrorAction SilentlyContinue; exit 3 }
Start-Sleep -Milliseconds 2500
$r = New-Object Win+RECT; [void][Win]::GetWindowRect($h, [ref]$r)
$w = $r.Right - $r.Left; $ht = $r.Bottom - $r.Top
$bmp = New-Object System.Drawing.Bitmap $w, $ht
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
$ok = [Win]::PrintWindow($h, $hdc, 2)   # PW_RENDERFULLCONTENT
$g.ReleaseHdc($hdc)
$bmp.Save($out, [System.Drawing.Imaging.ImageFormat]::Png); $g.Dispose(); $bmp.Dispose()
"PrintWindow ok={0}; {1}x{2}; saved {3} bytes" -f $ok, $w, $ht, (Get-Item $out).Length
if ($args.Count -lt 4) { Stop-Process -Id $p.Id -ErrorAction SilentlyContinue } else { "left open, pid {0}" -f $p.Id }
