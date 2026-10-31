Add-Type -AssemblyName System.Drawing

$size = 1024
$bitmap = New-Object System.Drawing.Bitmap($size, $size)
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality

$ground = [System.Drawing.ColorTranslator]::FromHtml('#0A0B0C')
$bone = [System.Drawing.ColorTranslator]::FromHtml('#EDEAE3')
$vermilion = [System.Drawing.ColorTranslator]::FromHtml('#E23A1F')

$graphics.Clear($ground)

# Hairline ring: the Q's bowl.
$ringPen = New-Object System.Drawing.Pen($bone, 54)
$ringPen.LineJoin = [System.Drawing.Drawing2D.LineJoin]::Round
$ringRect = New-Object System.Drawing.RectangleF(212, 212, 600, 600)
$graphics.DrawEllipse($ringPen, $ringRect)

# Tail, drawn before the bars so the bars stay legible.
$tailPen = New-Object System.Drawing.Pen($vermilion, 54)
$tailPen.StartCap = [System.Drawing.Drawing2D.LineCap]::Round
$tailPen.EndCap = [System.Drawing.Drawing2D.LineCap]::Round
$graphics.DrawLine($tailPen, 706, 706, 852, 852)

# Pulsar bars inside the bowl.
$barHeights = @(96, 208, 316, 168, 84)
$barWidth = 34
$gap = 22
$total = ($barHeights.Count * $barWidth) + (($barHeights.Count - 1) * $gap)
$barBrush = New-Object System.Drawing.SolidBrush($vermilion)
$cursor = 512 - ($total / 2)
foreach ($height in $barHeights) {
    $graphics.FillRectangle($barBrush, [single]$cursor, [single](512 - ($height / 2)), [single]$barWidth, [single]$height)
    $cursor += $barWidth + $gap
}

$graphics.Dispose()
$bitmap.Save("$PSScriptRoot\icon-source.png", [System.Drawing.Imaging.ImageFormat]::Png)
$bitmap.Dispose()
Write-Output "wrote icon-source.png"
