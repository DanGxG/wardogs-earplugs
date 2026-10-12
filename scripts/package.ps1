param(
    [Parameter(Mandatory = $true)][string]$Executable,
    [Parameter(Mandatory = $true)][ValidatePattern('^v[0-9]+\.[0-9]+\.[0-9]+$')][string]$Tag,
    [switch]$RequireSignature
)
$ErrorActionPreference = 'Stop'
$signature = Get-AuthenticodeSignature -LiteralPath $Executable
if ($RequireSignature -and $signature.Status -ne 'Valid') { throw "Trusted Authenticode signature required; got $($signature.Status)." }
$suffix = if ($signature.Status -eq 'Valid') { 'windows-x64' } else { 'windows-x64-unsigned' }
New-Item -ItemType Directory -Path 'dist' -Force | Out-Null
$packageDirectory = Join-Path 'dist' "wardogs-earplugs-$Tag-$suffix"
New-Item -ItemType Directory -Path $packageDirectory -Force | Out-Null
$outputExe = Join-Path 'dist' "wardogs-earplugs-$Tag-$suffix.exe"
Copy-Item -LiteralPath $Executable -Destination $outputExe
Copy-Item -LiteralPath $Executable -Destination (Join-Path $packageDirectory 'wardogs-earplugs.exe')
Copy-Item -LiteralPath 'README.md','LICENSE','SIGNING.md','VALIDATION.md' -Destination $packageDirectory
$zipPath = "$packageDirectory.zip"
Compress-Archive -LiteralPath $packageDirectory -DestinationPath $zipPath -Force
$checksum = @($outputExe, $zipPath) | ForEach-Object {
    $hash = Get-FileHash -LiteralPath $_ -Algorithm SHA256
    "$($hash.Hash.ToLowerInvariant())  $([IO.Path]::GetFileName($_))"
}
$checksum | Set-Content -LiteralPath 'dist/SHA256SUMS.txt' -Encoding ascii
Write-Output "Signature: $($signature.Status)"
Get-Item -LiteralPath $outputExe,$zipPath | Select-Object Name,Length
