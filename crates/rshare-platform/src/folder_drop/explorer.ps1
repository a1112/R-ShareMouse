param([string]$Mode, [int]$PointX, [int]$PointY)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, WindowsBase
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class RShareFolderHit {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT point);
  [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr window, uint flag);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
}
'@
try { $null = [RShareFolderHit]::SetThreadDpiAwarenessContext([IntPtr](-4)) }
catch { $null = [RShareFolderHit]::SetProcessDPIAware() }
$point = New-Object RShareFolderHit+POINT
$point.X = $PointX; $point.Y = $PointY
$root = [RShareFolderHit]::GetAncestor([RShareFolderHit]::WindowFromPoint($point), 2)
$shell = New-Object -ComObject Shell.Application
$windows = @($shell.Windows() | Where-Object { [long]$_.HWND -eq $root.ToInt64() })
if ($windows.Count -eq 0) { throw 'Not an Explorer folder window' }
$element = [System.Windows.Automation.AutomationElement]::FromPoint((New-Object System.Windows.Point($PointX, $PointY)))
$walker = [System.Windows.Automation.TreeWalker]::ControlViewWalker
$itemName = $null
$content = $false
for ($depth = 0; $null -ne $element -and $depth -lt 32; $depth++) {
  $type = $element.Current.ControlType.ProgrammaticName
  if ($type -in @('ControlType.ToolBar', 'ControlType.Edit', 'ControlType.Menu', 'ControlType.Tree', 'ControlType.TreeItem')) { throw 'Not a folder content area' }
  if ($type -in @('ControlType.ListItem', 'ControlType.DataItem')) { $itemName = $element.Current.Name }
  if ($type -in @('ControlType.List', 'ControlType.DataGrid')) { $content = $true; break }
  $element = $walker.GetParent($element)
}
if (-not $content) { throw 'Not a folder content area' }
# Multiple tabs can expose the same top-level HWND. Only the visible tab's
# shell view HWND contains the hit element; match its ancestor HWND as well.
$viewHandles = New-Object 'System.Collections.Generic.HashSet[long]'
$element = [System.Windows.Automation.AutomationElement]::FromPoint((New-Object System.Windows.Point($PointX, $PointY)))
for ($depth = 0; $null -ne $element -and $depth -lt 32; $depth++) {
  $null = $viewHandles.Add([long]$element.Current.NativeWindowHandle)
  $element = $walker.GetParent($element)
}
$candidates = @($windows | Where-Object {
  try { $viewHandles.Contains([long]$_.Document.Application.HWND) } catch { $false }
})
# Ambiguous tabs are rejected rather than writing into a hidden folder.
if ($windows.Count -ne 1 -and $candidates.Count -ne 1) { throw 'Ambiguous Explorer tabs' }
$window = if ($windows.Count -eq 1) { $windows[0] } else { $candidates[0] }
$folder = $window.Document.Folder
$folderPath = [string]$folder.Self.Path
if (-not [System.IO.Directory]::Exists($folderPath)) { throw 'Virtual folders are unsupported' }
$paths = New-Object 'System.Collections.Generic.List[string]'
if ($Mode -eq 'source') {
  if ([string]::IsNullOrEmpty($itemName)) { throw 'Source is not a file item' }
  $selected = @($window.Document.SelectedItems())
  $hitSelected = @($selected | Where-Object { $_.Name -eq $itemName -or [System.IO.Path]::GetFileName($_.Path) -eq $itemName })
  if ($hitSelected.Count -ne 1) { throw 'Source item is not selected' }
  foreach ($item in $selected) {
    if ($paths.Count -ge 1024) { throw 'Too many files' }
    $path = [string]$item.Path
    if (-not [System.IO.File]::Exists($path) -and -not [System.IO.Directory]::Exists($path)) { throw 'Virtual items are unsupported' }
    $paths.Add($path)
  }
} elseif ($Mode -eq 'target') {
  if (-not [string]::IsNullOrEmpty($itemName)) {
    $items = @($folder.Items() | Where-Object { $_.Name -eq $itemName -or [System.IO.Path]::GetFileName($_.Path) -eq $itemName })
    if ($items.Count -ne 1 -or -not $items[0].IsFolder -or -not [System.IO.Directory]::Exists($items[0].Path)) { throw 'Target item is not a filesystem folder' }
    $folderPath = [string]$items[0].Path
  }
  $paths.Add($folderPath)
} else { throw 'Unknown operation' }
ConvertTo-Json -InputObject @($paths.ToArray()) -Compress
