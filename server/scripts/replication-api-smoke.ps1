$ErrorActionPreference = 'Stop'

$suffix = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$primaryLogin = "primary$suffix"
$mirrorLogin = "mirror$suffix"
$password = 'replication-test-123'

$primaryAuth = Invoke-RestMethod -Method Post `
    -Uri 'http://127.0.0.1:18090/api/auth/register' `
    -ContentType 'application/json' `
    -Body (@{
        login = $primaryLogin
        password = $password
        displayName = 'Primary User'
        publicKey = $null
    } | ConvertTo-Json)

$serverId = [Guid]::NewGuid()
$registration = Invoke-RestMethod -Method Post `
    -Uri 'http://127.0.0.1:18090/api/servers/become-host' `
    -Headers @{ Authorization = "Bearer $($primaryAuth.accessToken)" } `
    -ContentType 'application/json' `
    -Body (@{
        serverId = $serverId
        name = 'Mirror'
        baseUrl = 'http://192.168.0.11:18091'
        publicKey = $null
    } | ConvertTo-Json)

Invoke-RestMethod -Method Post `
    -Uri 'http://127.0.0.1:18091/api/replication/configure' `
    -Headers @{ 'X-Local-Host-Secret' = 'local-test-secret' } `
    -ContentType 'application/json' `
    -Body (@{
        serverId = $registration.id
        serverName = 'Mirror'
        baseUrl = 'http://192.168.0.11:18091'
        upstreamUrl = 'http://127.0.0.1:18090'
        replicationKey = $registration.replicationKey
    } | ConvertTo-Json)

$primaryOnMirror = $null
for ($attempt = 0; $attempt -lt 30; $attempt++) {
    Start-Sleep -Seconds 1
    try {
        $primaryOnMirror = Invoke-RestMethod -Method Post `
            -Uri 'http://127.0.0.1:18091/api/auth/login' `
            -ContentType 'application/json' `
            -Body (@{ login = $primaryLogin; password = $password } | ConvertTo-Json)
        break
    } catch {
    }
}
if ($null -eq $primaryOnMirror) {
    throw 'Primary user was not replicated to mirror'
}

$mirrorAuth = Invoke-RestMethod -Method Post `
    -Uri 'http://127.0.0.1:18091/api/auth/register' `
    -ContentType 'application/json' `
    -Body (@{
        login = $mirrorLogin
        password = $password
        displayName = 'Mirror User'
        publicKey = $null
    } | ConvertTo-Json)

$mirrorHeaders = @{ Authorization = "Bearer $($mirrorAuth.accessToken)" }
$keyId = "replication-key-$suffix"
Invoke-RestMethod -Method Post `
    -Uri 'http://127.0.0.1:18091/api/keys/public' `
    -Headers $mirrorHeaders `
    -ContentType 'application/json' `
    -Body (@{
        keyId = $keyId
        algorithm = 'ECDH-P256-AESGCM-MVP'
        publicKey = 'replicated-public-key'
    } | ConvertTo-Json) | Out-Null
Invoke-RestMethod -Method Post `
    -Uri 'http://127.0.0.1:18091/api/keys/backup/encrypted' `
    -Headers $mirrorHeaders `
    -ContentType 'application/json' `
    -Body (@{
        keyId = $keyId
        algorithm = 'ECDH-P256-AESGCM-MVP'
        publicKey = 'replicated-public-key'
        salt = 'c2FsdA=='
        nonce = 'bm9uY2U='
        kdf = 'PBKDF2-SHA256:200000:AES-256-GCM'
        encryptedPrivateKey = 'encrypted-private-key'
    } | ConvertTo-Json)
$directChat = Invoke-RestMethod -Method Post `
    -Uri 'http://127.0.0.1:18091/api/chats/direct' `
    -Headers $mirrorHeaders `
    -ContentType 'application/json' `
    -Body (@{ userId = $primaryAuth.userId } | ConvertTo-Json)

$mirrorOnPrimary = $null
for ($attempt = 0; $attempt -lt 30; $attempt++) {
    Start-Sleep -Seconds 1
    try {
        $mirrorOnPrimary = Invoke-RestMethod -Method Post `
            -Uri 'http://127.0.0.1:18090/api/auth/login' `
            -ContentType 'application/json' `
            -Body (@{ login = $mirrorLogin; password = $password } | ConvertTo-Json)
        break
    } catch {
    }
}
if ($null -eq $mirrorOnPrimary) {
    throw 'Mirror user was not replicated to primary'
}

$search = Invoke-RestMethod `
    -Uri "http://127.0.0.1:18090/api/users/search?q=$mirrorLogin" `
    -Headers @{ Authorization = "Bearer $($mirrorOnPrimary.accessToken)" }
$primaryMirrorHeaders = @{ Authorization = "Bearer $($mirrorOnPrimary.accessToken)" }
$replicatedKey = Invoke-RestMethod `
    -Uri "http://127.0.0.1:18090/api/keys/public/$($mirrorAuth.userId)" `
    -Headers $primaryMirrorHeaders
$replicatedBackup = Invoke-RestMethod `
    -Uri 'http://127.0.0.1:18090/api/keys/backup/encrypted' `
    -Headers $primaryMirrorHeaders
$replicatedChats = Invoke-RestMethod `
    -Uri 'http://127.0.0.1:18090/api/chats' `
    -Headers $primaryMirrorHeaders

$socket = [System.Net.WebSockets.ClientWebSocket]::new()
$socketUri = "ws://127.0.0.1:18091/ws?token=" + [Uri]::EscapeDataString($mirrorAuth.accessToken)
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
        chatId = $directChat.id
        encryptedContent = 'replicated-ciphertext'
        nonce = 'replicated-nonce'
        encryptionKeyId = $keyId
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
$socket.Dispose()

$primaryOnMirrorHeaders = @{ Authorization = "Bearer $($primaryOnMirror.accessToken)" }
$reactionJson = '{"reaction":"\u2764"}'
Invoke-RestMethod -Method Put "http://127.0.0.1:18091/api/messages/$messageId/reaction" `
    -Headers $primaryOnMirrorHeaders -ContentType 'application/json; charset=utf-8' `
    -Body $reactionJson | Out-Null
Invoke-RestMethod -Method Put "http://127.0.0.1:18091/api/messages/$messageId/pin" `
    -Headers $primaryOnMirrorHeaders -ContentType 'application/json' `
    -Body (@{ pinned = $true } | ConvertTo-Json) | Out-Null

$interactionOnPrimary = $null
for ($attempt = 0; $attempt -lt 30; $attempt++) {
    Start-Sleep -Seconds 1
    $history = @(Invoke-RestMethod `
        -Uri "http://127.0.0.1:18090/api/chats/$($directChat.id)/messages" `
        -Headers $primaryMirrorHeaders)
    $candidate = $history | Where-Object { $_.id -eq $messageId } | Select-Object -First 1
    if ($null -ne $candidate -and $candidate.pinned -and @($candidate.reactions).Count -eq 1) {
        $interactionOnPrimary = $candidate
        break
    }
}
if ($null -eq $interactionOnPrimary) {
    throw 'Message interactions were not replicated to primary'
}

Invoke-RestMethod -Method Delete "http://127.0.0.1:18091/api/messages/$messageId" `
    -Headers $mirrorHeaders | Out-Null
$deletionReplicated = $false
for ($attempt = 0; $attempt -lt 30; $attempt++) {
    Start-Sleep -Seconds 1
    $history = @(Invoke-RestMethod `
        -Uri "http://127.0.0.1:18090/api/chats/$($directChat.id)/messages" `
        -Headers $primaryMirrorHeaders)
    if (-not ($history | Where-Object { $_.id -eq $messageId })) {
        $deletionReplicated = $true
        break
    }
}
if (!$deletionReplicated) {
    throw 'Message deletion was not replicated to primary'
}
$status = Invoke-RestMethod -Uri 'http://127.0.0.1:18091/api/replication/status'

[pscustomobject]@{
    RegisteredRole = $registration.role
    PrimaryUrl = $registration.primaryUrl
    PrimaryUserLoginOnMirror = $primaryOnMirror.login -eq $primaryLogin
    MirrorUserLoginOnPrimary = $mirrorOnPrimary.login -eq $mirrorLogin
    SearchResultCount = @($search).Count
    PublicKeyReplicated = $replicatedKey.keyId -eq $keyId
    EncryptedBackupReplicated = $replicatedBackup.keyId -eq $keyId
    DirectChatReplicated = @($replicatedChats).id -contains $directChat.id
    MessageReplicated = $interactionOnPrimary.id -eq $messageId
    ReactionReplicated = $interactionOnPrimary.reactions[0].count -eq 1
    PinReplicated = $interactionOnPrimary.pinned
    DeletionReplicated = $deletionReplicated
    MirrorReplicationState = $status.state
}
