[CmdletBinding()]
param(
    [string]$SourcePng = (Join-Path (Split-Path -Parent $PSScriptRoot) 'Turat_bg.png'),
    [string]$OutputIco = (Join-Path (Split-Path -Parent $PSScriptRoot) 'client\windows\Assets\Turat.ico')
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

if (-not (Test-Path -LiteralPath $SourcePng)) {
    throw "Исходный PNG логотипа не найден: $SourcePng"
}

$sizes = @(16, 20, 24, 32, 40, 48, 64, 96, 128, 256)
$frames = [System.Collections.Generic.List[object]]::new()
$source = [System.Drawing.Image]::FromFile($SourcePng)

try {
    foreach ($size in $sizes) {
        $bitmap = [System.Drawing.Bitmap]::new($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
        try {
            $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
            try {
                $graphics.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
                $graphics.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
                $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
                $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
                $graphics.DrawImage($source, 0, 0, $size, $size)
            }
            finally {
                $graphics.Dispose()
            }

            $stream = [System.IO.MemoryStream]::new()
            try {
                $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
                $frames.Add([pscustomobject]@{
                    Size = $size
                    Bytes = $stream.ToArray()
                })
            }
            finally {
                $stream.Dispose()
            }
        }
        finally {
            $bitmap.Dispose()
        }
    }
}
finally {
    $source.Dispose()
}

$outputDirectory = Split-Path -Parent $OutputIco
New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
$file = [System.IO.File]::Open($OutputIco, [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write)
$writer = [System.IO.BinaryWriter]::new($file)

try {
    # ICONDIR
    $writer.Write([uint16]0)
    $writer.Write([uint16]1)
    $writer.Write([uint16]$frames.Count)

    $offset = 6 + (16 * $frames.Count)
    foreach ($frame in $frames) {
        # ICONDIRENTRY. Windows Vista+ supports PNG-compressed icon frames.
        $dimension = if ($frame.Size -eq 256) { 0 } else { $frame.Size }
        $writer.Write([byte]$dimension)
        $writer.Write([byte]$dimension)
        $writer.Write([byte]0)
        $writer.Write([byte]0)
        $writer.Write([uint16]1)
        $writer.Write([uint16]32)
        $writer.Write([uint32]$frame.Bytes.Length)
        $writer.Write([uint32]$offset)
        $offset += $frame.Bytes.Length
    }

    foreach ($frame in $frames) {
        $writer.Write([byte[]]$frame.Bytes)
    }
}
finally {
    $writer.Dispose()
    $file.Dispose()
}

Write-Host "Windows ICO собран без преобразования через HICON: $OutputIco"
