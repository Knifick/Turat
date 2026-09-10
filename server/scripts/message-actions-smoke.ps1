$ErrorActionPreference = 'Stop'

$testDirectory = Join-Path $env:TEMP 'turattext-message-actions-test'
New-Item -ItemType Directory -Force $testDirectory | Out-Null
$stdout = Join-Path $testDirectory 'server.out.log'
$stderr = Join-Path $testDirectory 'server.err.log'
Remove-Item $stdout, $stderr -Force -ErrorAction SilentlyContinue
$jar = Get-ChildItem "$PSScriptRoot\..\build\libs\turattext-server-*.jar" | Select-Object -First 1
$arguments = @(
    '-jar', $jar.FullName,
    '--spring.profiles.active=dev',
    '--server.port=18093',
    '--spring.datasource.url=jdbc:h2:mem:actions;MODE=PostgreSQL;DATABASE_TO_LOWER=TRUE;CASE_INSENSITIVE_IDENTIFIERS=TRUE',
    '--spring.h2.console.enabled=false',
    '--turattext.backup.enabled=false',
    '--turattext.server.self-register=false',
    '--turattext.jwt.secret=integration-test-secret-at-least-32-bytes-long'
)
$serverProcess = Start-Process java -ArgumentList $arguments -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput $stdout -RedirectStandardError $stderr

try {
    $ready = $false
    for ($attempt = 0; $attempt -lt 45; $attempt++) {
        Start-Sleep -Seconds 1
        try {
            Invoke-RestMethod 'http://127.0.0.1:18093/api/servers/health' | Out-Null
            $ready = $true
            break
        } catch {
        }
    }
    if (!$ready) { throw 'Server did not start' }

    $suffix = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $password = 'actions-test-123'
    $first = Invoke-RestMethod -Method Post 'http://127.0.0.1:18093/api/auth/register' `
        -ContentType 'application/json' `
        -Body (@{ login = "actiona$suffix"; password = $password; displayName = 'Action A' } | ConvertTo-Json)
    $second = Invoke-RestMethod -Method Post 'http://127.0.0.1:18093/api/auth/register' `
        -ContentType 'application/json' `
        -Body (@{ login = "actionb$suffix"; password = $password; displayName = 'Action B' } | ConvertTo-Json)
    $firstHeaders = @{ Authorization = "Bearer $($first.accessToken)" }
    $secondHeaders = @{ Authorization = "Bearer $($second.accessToken)" }
    $chat = Invoke-RestMethod -Method Post 'http://127.0.0.1:18093/api/chats/direct' `
        -Headers $firstHeaders -ContentType 'application/json' `
        -Body (@{ userId = $second.userId } | ConvertTo-Json)

    $socket = [System.Net.WebSockets.ClientWebSocket]::new()
    $socketUri = "ws://127.0.0.1:18093/ws?token=" + [Uri]::EscapeDataString($first.accessToken)
    [void]$socket.ConnectAsync([Uri]$socketUri, [Threading.CancellationToken]::None).GetAwaiter().GetResult()

    function Receive-Json([System.Net.WebSockets.ClientWebSocket]$source) {
        $buffer = New-Object byte[] 65536
        $segment = [ArraySegment[byte]]::new($buffer)
        $result = $source.ReceiveAsync($segment, [Threading.CancellationToken]::None).GetAwaiter().GetResult()
        return [Text.Encoding]::UTF8.GetString($buffer, 0, $result.Count) | ConvertFrom-Json
    }

    Receive-Json $socket | Out-Null
    $send = @{
        type = 'message.send'
        payload = @{
            chatId = $chat.id
            encryptedContent = 'ciphertext'
            nonce = 'nonce'
            encryptionKeyId = 'key'
            replyToMessageId = $null
        }
    } | ConvertTo-Json -Depth 5 -Compress
    $bytes = [Text.Encoding]::UTF8.GetBytes($send)
    [void]$socket.SendAsync(
        [ArraySegment[byte]]::new($bytes),
        [System.Net.WebSockets.WebSocketMessageType]::Text,
        $true,
        [Threading.CancellationToken]::None
    ).GetAwaiter().GetResult()
    do {
        $messageEvent = Receive-Json $socket
    } while ($messageEvent.type -ne 'message.received')
    $messageId = $messageEvent.payload.id

    Invoke-RestMethod 'http://127.0.0.1:18093/api/chats' -Headers $secondHeaders | Out-Null
    $reactionJson = '{"reaction":"\u2764"}'
    $reaction = Invoke-RestMethod -Method Put "http://127.0.0.1:18093/api/messages/$messageId/reaction" `
        -Headers $secondHeaders -ContentType 'application/json; charset=utf-8' `
        -Body $reactionJson
    $pin = Invoke-RestMethod -Method Put "http://127.0.0.1:18093/api/messages/$messageId/pin" `
        -Headers $secondHeaders -ContentType 'application/json' `
        -Body (@{ pinned = $true } | ConvertTo-Json)
    $history = Invoke-RestMethod "http://127.0.0.1:18093/api/chats/$($chat.id)/messages" `
        -Headers $secondHeaders
    $deleted = Invoke-RestMethod -Method Delete "http://127.0.0.1:18093/api/messages/$messageId" `
        -Headers $firstHeaders
    $historyAfterDelete = Invoke-RestMethod "http://127.0.0.1:18093/api/chats/$($chat.id)/messages" `
        -Headers $secondHeaders

    [pscustomobject]@{
        WebSocketMessage = $messageEvent.type -eq 'message.received'
        ReactionCount = $reaction.reactions[0].count
        ReactionMine = $reaction.reactions[0].reactedByMe
        Pinned = $pin.pinned
        HistoryPinned = $history[0].pinned
        Deleted = $deleted.deleted
        HistoryAfterDelete = @($historyAfterDelete).Count
    }
    $socket.Dispose()
} finally {
    Stop-Process -Id $serverProcess.Id -Force -ErrorAction SilentlyContinue
}
