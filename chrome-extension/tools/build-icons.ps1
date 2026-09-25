#Requires -Version 5.1
# Rasterize the same simple geometry as icons/sonos-tab-caster.svg on Windows.
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
$directory=[IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../icons'))
foreach($size in @(16,32,48,128)){
    $bitmap=[Drawing.Bitmap]::new($size,$size)
    $graphics=[Drawing.Graphics]::FromImage($bitmap)
    $background=[Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#153848'))
    $pen=[Drawing.Pen]::new([Drawing.ColorTranslator]::FromHtml('#7be3c5'),12)
    $path=[Drawing.Drawing2D.GraphicsPath]::new()
    try{
        $graphics.SmoothingMode=[Drawing.Drawing2D.SmoothingMode]::AntiAlias
        $graphics.ScaleTransform($size/128.0,$size/128.0)
        $path.AddArc(4,4,56,56,180,90);$path.AddArc(68,4,56,56,270,90);$path.AddArc(68,68,56,56,0,90);$path.AddArc(4,68,56,56,90,90);$path.CloseFigure()
        $graphics.FillPath($background,$path)
        $pen.StartCap=[Drawing.Drawing2D.LineCap]::Round;$pen.EndCap=[Drawing.Drawing2D.LineCap]::Round
        $graphics.DrawLine($pen,34,53,34,75);$graphics.DrawLine($pen,54,36,54,92);$graphics.DrawLine($pen,74,46,74,82);$graphics.DrawLine($pen,94,56,94,72)
        $bitmap.Save((Join-Path $directory ("icon-$size.png")),[Drawing.Imaging.ImageFormat]::Png)
    }finally{$path.Dispose();$pen.Dispose();$background.Dispose();$graphics.Dispose();$bitmap.Dispose()}
}
