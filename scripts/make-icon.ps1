$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
$assetDirectory = Join-Path (Split-Path -Parent $PSScriptRoot) 'assets'
$frames = @()
function Rounded-Rectangle([float]$X, [float]$Y, [float]$Width, [float]$Height, [float]$Radius) {
    $path = [Drawing.Drawing2D.GraphicsPath]::new()
    $diameter = $Radius * 2
    $path.AddArc($X, $Y, $diameter, $diameter, 180, 90)
    $path.AddArc($X + $Width - $diameter, $Y, $diameter, $diameter, 270, 90)
    $path.AddArc($X + $Width - $diameter, $Y + $Height - $diameter, $diameter, $diameter, 0, 90)
    $path.AddArc($X, $Y + $Height - $diameter, $diameter, $diameter, 90, 90)
    $path.CloseFigure()
    return ,$path
}
# Native drawing of assets/earplugs.svg; this script is not part of app startup.
foreach ($size in @(16, 24, 32, 48, 64, 256)) {
    $bitmap = [Drawing.Bitmap]::new($size, $size, [Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    $background = [Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#10252d'))
    $mint = [Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#83e6c2'))
    $cord = [Drawing.Pen]::new($mint, 3.5)
    $groove = [Drawing.Pen]::new($background, 2)
    $path = [Drawing.Drawing2D.GraphicsPath]::new()
    $stream = [IO.MemoryStream]::new()
    try {
        $graphics.Clear([Drawing.Color]::Transparent)
        $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.ScaleTransform($size / 64.0, $size / 64.0)
        $rounded = Rounded-Rectangle 4 4 56 56 16
        try { $graphics.FillPath($background, $rounded) } finally { $rounded.Dispose() }
        $cord.StartCap = $cord.EndCap = [Drawing.Drawing2D.LineCap]::Round
        $groove.StartCap = $groove.EndCap = [Drawing.Drawing2D.LineCap]::Round
        $path.AddLine(24, 34, 24, 42)
        $path.AddBezier(24, 42, 24, 54, 40, 54, 40, 42)
        $path.AddLine(40, 42, 40, 34)
        $graphics.DrawPath($cord, $path)
        foreach ($x in @(18, 34)) {
            $rounded = Rounded-Rectangle $x 13 12 25 6
            try { $graphics.FillPath($mint, $rounded) } finally { $rounded.Dispose() }
            foreach ($y in @(22, 27)) { $graphics.DrawLine($groove, $x + 3, $y, $x + 9, $y) }
        }
        $bitmap.Save($stream, [Drawing.Imaging.ImageFormat]::Png)
        $frames += [pscustomobject]@{ Size = $size; Bytes = $stream.ToArray() }
    } finally {
        $stream.Dispose(); $path.Dispose(); $groove.Dispose(); $cord.Dispose()
        $mint.Dispose(); $background.Dispose(); $graphics.Dispose(); $bitmap.Dispose()
    }
}
function Write-Icon([string]$Name, [object[]]$IconFrames) {
    $output = [IO.File]::Create((Join-Path $assetDirectory $Name))
    $writer = [IO.BinaryWriter]::new($output)
    try {
        $writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$IconFrames.Count)
        $offset = 6 + 16 * $IconFrames.Count
        foreach ($frame in $IconFrames) {
            $dimension = if ($frame.Size -eq 256) { 0 } else { $frame.Size }
            $writer.Write([byte]$dimension); $writer.Write([byte]$dimension)
            $writer.Write([byte]0); $writer.Write([byte]0)
            $writer.Write([uint16]1); $writer.Write([uint16]32)
            $writer.Write([uint32]$frame.Bytes.Length); $writer.Write([uint32]$offset)
            $offset += $frame.Bytes.Length
        }
        foreach ($frame in $IconFrames) { $writer.Write([byte[]]$frame.Bytes) }
    } finally { $writer.Dispose(); $output.Dispose() }
}
Write-Icon 'earplugs.ico' $frames
Write-Icon 'earplugs-small.ico' @($frames[0])
Get-Item -LiteralPath (Join-Path $assetDirectory 'earplugs.ico'),(Join-Path $assetDirectory 'earplugs-small.ico') | Select-Object Name, Length
