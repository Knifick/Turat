$ErrorActionPreference = 'Stop'

$sourceDatabase = Join-Path $env:APPDATA 'TuratText\host\data\turattext-host.mv.db'
if (!(Test-Path $sourceDatabase)) {
    throw "Existing host database was not found: $sourceDatabase"
}

$testDirectory = Join-Path $env:TEMP 'turattext-existing-db-test'
New-Item -ItemType Directory -Force $testDirectory | Out-Null
$targetBase = Join-Path $testDirectory 'turattext-host'
Copy-Item -LiteralPath $sourceDatabase -Destination ($targetBase + '.mv.db') -Force
$stdout = Join-Path $testDirectory 'out.log'
$stderr = Join-Path $testDirectory 'err.log'
Remove-Item $stdout, $stderr -Force -ErrorAction SilentlyContinue
$jar = Get-ChildItem "$PSScriptRoot\..\build\libs\turattext-server-*.jar" | Select-Object -First 1
$databaseUrl = $targetBase.Replace('\', '/')
$arguments = @(
    '-jar', $jar.FullName,
    '--spring.profiles.active=dev',
    '--server.port=18094',
    "--spring.datasource.url=jdbc:h2:file:$databaseUrl;MODE=PostgreSQL;DATABASE_TO_LOWER=TRUE;CASE_INSENSITIVE_IDENTIFIERS=TRUE",
    '--spring.h2.console.enabled=false',
    '--turattext.backup.enabled=false',
    '--turattext.server.self-register=false'
)
$serverProcess = Start-Process java -ArgumentList $arguments -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput $stdout -RedirectStandardError $stderr

try {
    $ready = $false
    for ($attempt = 0; $attempt -lt 45; $attempt++) {
        Start-Sleep -Seconds 1
        try {
            $health = Invoke-RestMethod 'http://127.0.0.1:18094/api/servers/health'
            $ready = $true
            break
        } catch {
        }
    }
    $schemaErrors = Select-String -Path $stdout, $stderr `
        -Pattern 'SchemaManagementException|CommandAcceptanceException|could not execute statement|JdbcSQL' `
        -CaseSensitive:$false
    [pscustomobject]@{
        Started = $ready
        Health = $health.status
        SchemaErrorCount = @($schemaErrors).Count
    }
} finally {
    Stop-Process -Id $serverProcess.Id -Force -ErrorAction SilentlyContinue
}
