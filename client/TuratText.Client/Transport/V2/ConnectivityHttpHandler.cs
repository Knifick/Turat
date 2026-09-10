using System.Collections.Concurrent;
using System.Net;
using System.Net.Sockets;
using System.Net.Http.Headers;
using System.Security.Cryptography;

namespace TuratText.Client.Transport.V2;

public sealed class ConnectivityHttpHandler : DelegatingHandler
{
    private readonly RelayDescriptorSource _relays;
    private readonly MetadataProtectionSettings _metadataProtection;
    private readonly ConcurrentDictionary<string, string> _preferredRelay = new(StringComparer.OrdinalIgnoreCase);
    private readonly ConcurrentDictionary<string, HttpMessageInvoker> _relayInvokers = new(StringComparer.Ordinal);

    public ConnectivityHttpHandler(RelayDescriptorSource relays, MetadataProtectionSettings metadataProtection)
    {
        _relays = relays;
        _metadataProtection = metadataProtection;
        var sockets = new SocketsHttpHandler
        {
            AutomaticDecompression = DecompressionMethods.All,
            PooledConnectionLifetime = TimeSpan.FromMinutes(5),
            ConnectTimeout = TimeSpan.FromSeconds(12),
            EnableMultipleHttp2Connections = true,
            ConnectCallback = ConnectAsync
        };
        InnerHandler = sockets;
    }

    public string LastRoute { get; private set; } = "direct";

    protected override async Task<HttpResponseMessage> SendAsync(
        HttpRequestMessage request,
        CancellationToken cancellationToken)
    {
        if (request.RequestUri is { } uri
            && (uri.Scheme == Uri.UriSchemeHttp || uri.IsLoopback))
        {
            request.VersionPolicy = HttpVersionPolicy.RequestVersionOrLower;
        }
        BufferedRequest buffered = await BufferedRequest.CreateAsync(request, cancellationToken);
        try
        {
            HttpResponseMessage direct = await base.SendAsync(request, cancellationToken);
            if ((int)direct.StatusCode is not (502 or 503 or 504)) return direct;
            RelayDescriptor? relay = await SelectRelayAsync(request.RequestUri, cancellationToken);
            if (relay is null) return direct;
            direct.Dispose();
            return await SendViaRelayAsync(buffered, relay, cancellationToken);
        }
        catch (HttpRequestException)
        {
            RelayDescriptor? relay = await SelectRelayAsync(request.RequestUri, cancellationToken);
            if (relay is null) throw;
            return await SendViaRelayAsync(buffered, relay, cancellationToken);
        }
    }

    private async ValueTask<Stream> ConnectAsync(
        SocketsHttpConnectionContext context,
        CancellationToken cancellationToken)
    {
        string targetKey = context.DnsEndPoint.Host + ":" + context.DnsEndPoint.Port;
        IReadOnlyList<RelayDescriptor> matching = (await _relays.LoadAsync(cancellationToken))
            .Where(value => value.TargetPort == context.DnsEndPoint.Port
                            && string.Equals(value.TargetHost, context.DnsEndPoint.Host, StringComparison.OrdinalIgnoreCase))
            .ToList();
        MetadataProtectionMode mode = await _metadataProtection.GetAsync(cancellationToken);
        if (mode == MetadataProtectionMode.HighPrivacy)
        {
            foreach (RelayDescriptor relay in matching.OrderBy(_ => RandomNumberGenerator.GetInt32(int.MaxValue)))
            {
                Stream? privateStream = await TryConnectAsync(
                    new DnsEndPoint(relay.ConnectHost, relay.ConnectPort), cancellationToken);
                if (privateStream is null) continue;
                _preferredRelay[targetKey] = relay.RelayId;
                LastRoute = "relay:" + relay.RelayId;
                return privateStream;
            }
        }
        if (_preferredRelay.TryGetValue(targetKey, out string? preferredId))
        {
            RelayDescriptor? preferred = matching.FirstOrDefault(value => value.RelayId == preferredId);
            if (preferred is not null)
            {
                Stream? preferredStream = await TryConnectAsync(
                    new DnsEndPoint(preferred.ConnectHost, preferred.ConnectPort), cancellationToken);
                if (preferredStream is not null)
                {
                    LastRoute = "relay:" + preferred.RelayId;
                    return preferredStream;
                }
                _preferredRelay.TryRemove(targetKey, out _);
            }
        }

        Stream? direct = await TryConnectAsync(context.DnsEndPoint, cancellationToken);
        if (direct is not null)
        {
            LastRoute = "direct";
            return direct;
        }
        foreach (RelayDescriptor relay in matching)
        {
            Stream? stream = await TryConnectAsync(
                new DnsEndPoint(relay.ConnectHost, relay.ConnectPort), cancellationToken);
            if (stream is null) continue;
            _preferredRelay[targetKey] = relay.RelayId;
            LastRoute = "relay:" + relay.RelayId;
            return stream;
        }
        throw new HttpRequestException($"No direct or signed relay route is reachable for {targetKey}");
    }

    private static async Task<Stream?> TryConnectAsync(DnsEndPoint endpoint, CancellationToken cancellationToken)
    {
        using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timeout.CancelAfter(TimeSpan.FromSeconds(4));
        var socket = new Socket(SocketType.Stream, ProtocolType.Tcp) { NoDelay = true };
        try
        {
            await socket.ConnectAsync(endpoint, timeout.Token);
            return new NetworkStream(socket, ownsSocket: true);
        }
        catch (Exception exception) when (exception is SocketException or OperationCanceledException)
        {
            socket.Dispose();
            cancellationToken.ThrowIfCancellationRequested();
            return null;
        }
    }

    private async Task<RelayDescriptor?> SelectRelayAsync(Uri? uri, CancellationToken cancellationToken)
    {
        if (uri is null) return null;
        int port = uri.IsDefaultPort ? (uri.Scheme == Uri.UriSchemeHttps ? 443 : 80) : uri.Port;
        IReadOnlyList<RelayDescriptor> values = (await _relays.LoadAsync(cancellationToken))
            .Where(value => value.TargetPort == port
                            && string.Equals(value.TargetHost, uri.Host, StringComparison.OrdinalIgnoreCase))
            .ToList();
        string key = uri.Host + ":" + port;
        if (_preferredRelay.TryGetValue(key, out string? preferred))
            return values.FirstOrDefault(value => value.RelayId == preferred) ?? values.FirstOrDefault();
        return values.FirstOrDefault();
    }

    private async Task<HttpResponseMessage> SendViaRelayAsync(
        BufferedRequest buffered,
        RelayDescriptor relay,
        CancellationToken cancellationToken)
    {
        HttpMessageInvoker invoker = _relayInvokers.GetOrAdd(relay.RelayId, _ =>
        {
            var sockets = new SocketsHttpHandler
            {
                AutomaticDecompression = DecompressionMethods.All,
                PooledConnectionLifetime = TimeSpan.FromMinutes(5),
                ConnectTimeout = TimeSpan.FromSeconds(8),
                ConnectCallback = async (_, token) =>
                    await TryConnectAsync(new DnsEndPoint(relay.ConnectHost, relay.ConnectPort), token)
                    ?? throw new HttpRequestException("Signed relay is unreachable")
            };
            return new HttpMessageInvoker(sockets, disposeHandler: true);
        });
        using HttpRequestMessage retry = buffered.Create();
        HttpResponseMessage response = await invoker.SendAsync(retry, cancellationToken);
        string targetKey = relay.TargetHost + ":" + relay.TargetPort;
        _preferredRelay[targetKey] = relay.RelayId;
        LastRoute = "relay:" + relay.RelayId;
        return response;
    }

    protected override void Dispose(bool disposing)
    {
        if (disposing)
        {
            foreach (HttpMessageInvoker invoker in _relayInvokers.Values) invoker.Dispose();
            _relayInvokers.Clear();
        }
        base.Dispose(disposing);
    }

    private sealed class BufferedRequest
    {
        private readonly HttpMethod _method;
        private readonly Uri? _uri;
        private readonly Version _version;
        private readonly HttpVersionPolicy _versionPolicy;
        private readonly IReadOnlyList<KeyValuePair<string, IEnumerable<string>>> _headers;
        private readonly byte[]? _content;
        private readonly IReadOnlyList<KeyValuePair<string, IEnumerable<string>>> _contentHeaders;

        private BufferedRequest(
            HttpRequestMessage value,
            byte[]? content,
            IReadOnlyList<KeyValuePair<string, IEnumerable<string>>> contentHeaders)
        {
            _method = value.Method;
            _uri = value.RequestUri;
            _version = value.Version;
            _versionPolicy = value.VersionPolicy;
            _headers = value.Headers.Select(pair => new KeyValuePair<string, IEnumerable<string>>(pair.Key, pair.Value.ToArray())).ToList();
            _content = content;
            _contentHeaders = contentHeaders;
        }

        public static async Task<BufferedRequest> CreateAsync(
            HttpRequestMessage value,
            CancellationToken cancellationToken)
        {
            byte[]? content = value.Content is null
                ? null
                : await value.Content.ReadAsByteArrayAsync(cancellationToken);
            IReadOnlyList<KeyValuePair<string, IEnumerable<string>>> headers = value.Content?.Headers
                .Select(pair => new KeyValuePair<string, IEnumerable<string>>(pair.Key, pair.Value.ToArray())).ToList()
                ?? [];
            return new BufferedRequest(value, content, headers);
        }

        public HttpRequestMessage Create()
        {
            var value = new HttpRequestMessage(_method, _uri)
            {
                Version = _version,
                VersionPolicy = _versionPolicy
            };
            foreach (var header in _headers) value.Headers.TryAddWithoutValidation(header.Key, header.Value);
            if (_content is not null)
            {
                value.Content = new ByteArrayContent(_content);
                foreach (var header in _contentHeaders)
                    value.Content.Headers.TryAddWithoutValidation(header.Key, header.Value);
            }
            return value;
        }
    }
}
