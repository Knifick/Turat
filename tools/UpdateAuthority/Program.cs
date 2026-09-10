using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Diagnostics;
using Org.BouncyCastle.Crypto.Parameters;
using Org.BouncyCastle.Crypto.Signers;
using Org.BouncyCastle.Crypto.Utilities;
using Org.BouncyCastle.Security;
using Org.BouncyCastle.X509;
using TuratText.Client.Updates;

if (args.Length == 0)
{
    Usage();
    return 2;
}

try
{
    switch (args[0].ToLowerInvariant())
    {
        case "init" when args.Length is 2 or 3:
            InitializeAuthority(args[1], args.Length == 3 ? int.Parse(args[2]) : 2);
            break;
        case "sign" when args.Length == 4:
            SignManifest(args[1], args[2], args[3]);
            break;
        case "init-android" when args.Length == 2:
            InitializeAndroidSigning(args[1]);
            break;
        default:
            Usage();
            return 2;
    }
    return 0;
}
catch (Exception exception)
{
    Console.Error.WriteLine(exception.Message);
    return 1;
}

static void InitializeAuthority(string directory, int threshold)
{
    string fullDirectory = Path.GetFullPath(directory);
    if (Directory.Exists(fullDirectory) && Directory.EnumerateFileSystemEntries(fullDirectory).Any())
        throw new InvalidOperationException("Authority directory must be new or empty");
    if (threshold is < 1 or > 3) throw new ArgumentOutOfRangeException(nameof(threshold));
    Directory.CreateDirectory(fullDirectory);
    var random = new SecureRandom();
    var keys = new List<UpdateTrustKey>();
    for (int index = 1; index <= 3; index++)
    {
        var privateKey = new Ed25519PrivateKeyParameters(random);
        Ed25519PublicKeyParameters publicKey = privateKey.GeneratePublicKey();
        byte[] spki = SubjectPublicKeyInfoFactory.CreateSubjectPublicKeyInfo(publicKey).GetEncoded();
        string publicValue = Convert.ToBase64String(spki);
        string keyId = ThresholdUpdateVerifier.KeyId(publicValue);
        File.WriteAllBytes(Path.Combine(fullDirectory, keyId + ".key"), privateKey.GetEncoded());
        keys.Add(new UpdateTrustKey(keyId, "Ed25519", publicValue));
    }
    var root = new UpdateTrustRoot(1, threshold, keys);
    File.WriteAllText(
        Path.Combine(fullDirectory, "trust-root.json"),
        JsonSerializer.Serialize(root, JsonOptions(indented: true)),
        new UTF8Encoding(false));
    File.WriteAllText(
        Path.Combine(fullDirectory, "KEEP-OFFLINE.txt"),
        "These Ed25519 .key files authorize TuratText releases. Keep them offline and never copy them to a VPS.\n",
        new UTF8Encoding(false));
    Console.WriteLine($"Created 3-key authority with threshold {threshold} in {fullDirectory}");
}

static void SignManifest(string authorityDirectory, string specificationPath, string outputPath)
{
    string rootPath = Path.Combine(Path.GetFullPath(authorityDirectory), "trust-root.json");
    UpdateTrustRoot root = JsonSerializer.Deserialize<UpdateTrustRoot>(File.ReadAllText(rootPath), JsonOptions())
                           ?? throw new InvalidOperationException("Trust root is invalid");
    ReleaseSpecification spec = JsonSerializer.Deserialize<ReleaseSpecification>(
                                    File.ReadAllText(specificationPath),
                                    JsonOptions())
                                ?? throw new InvalidOperationException("Release specification is invalid");
    if (spec.ExpiresDays is < 1 or > 180 || spec.Artifacts.Count is < 1 or > 16)
        throw new InvalidOperationException("Release specification limits are invalid");
    long published = DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();
    var artifacts = new List<UpdateArtifact>();
    foreach (ReleaseArtifactSpecification item in spec.Artifacts)
    {
        byte[] content = File.ReadAllBytes(item.LocalPath);
        artifacts.Add(new UpdateArtifact(
            item.Platform,
            item.Architecture,
            item.Url,
            content.LongLength,
            Convert.ToHexString(SHA256.HashData(content)).ToLowerInvariant()));
    }
    var body = new UpdateManifestBody(
        2,
        spec.Channel,
        spec.Sequence,
        spec.ReleaseVersion,
        published,
        DateTimeOffset.FromUnixTimeMilliseconds(published).AddDays(spec.ExpiresDays).ToUnixTimeMilliseconds(),
        spec.NotesUrl,
        artifacts);
    string bodyJson = JsonSerializer.Serialize(body, JsonOptions());
    byte[] bodyBytes = Encoding.UTF8.GetBytes(bodyJson);
    var signatures = new List<UpdateManifestSignature>();
    foreach (UpdateTrustKey key in root.Keys.Take(root.Threshold))
    {
        byte[] seed = File.ReadAllBytes(Path.Combine(Path.GetFullPath(authorityDirectory), key.KeyId + ".key"));
        if (seed.Length != Ed25519PrivateKeyParameters.KeySize)
            throw new CryptographicException($"Private key {key.KeyId} is invalid");
        var privateKey = new Ed25519PrivateKeyParameters(seed, 0);
        var signer = new Ed25519Signer();
        signer.Init(true, privateKey);
        signer.BlockUpdate(bodyBytes, 0, bodyBytes.Length);
        signatures.Add(new UpdateManifestSignature(key.KeyId, "Ed25519", Convert.ToBase64String(signer.GenerateSignature())));
        CryptographicOperations.ZeroMemory(seed);
    }
    var manifest = new SignedUpdateManifest(body, bodyJson, signatures);
    _ = ThresholdUpdateVerifier.Verify(manifest, root);
    string destination = Path.GetFullPath(outputPath);
    Directory.CreateDirectory(Path.GetDirectoryName(destination)!);
    File.WriteAllText(destination, JsonSerializer.Serialize(manifest, JsonOptions(indented: true)), new UTF8Encoding(false));
    Console.WriteLine($"Signed update sequence {body.Sequence} with {signatures.Count} keys: {destination}");
}

static void InitializeAndroidSigning(string directory)
{
    string fullDirectory = Path.GetFullPath(directory);
    string keyStore = Path.Combine(fullDirectory, "turattext-release.keystore");
    string passwordFile = Path.Combine(fullDirectory, "android-signing-password.txt");
    if (File.Exists(keyStore) || File.Exists(passwordFile))
        throw new InvalidOperationException("Android signing authority already exists");
    Directory.CreateDirectory(fullDirectory);
    string password = Convert.ToBase64String(RandomNumberGenerator.GetBytes(36))
        .TrimEnd('=').Replace('+', '-').Replace('/', '_');
    string javaHome = Environment.GetEnvironmentVariable("JAVA_HOME") ?? "";
    string keytool = string.IsNullOrWhiteSpace(javaHome)
        ? "keytool"
        : Path.Combine(javaHome, "bin", OperatingSystem.IsWindows() ? "keytool.exe" : "keytool");
    var start = new ProcessStartInfo(keytool)
    {
        UseShellExecute = false,
        CreateNoWindow = true,
        WindowStyle = ProcessWindowStyle.Hidden
    };
    foreach (string argument in new[]
             {
                 "-genkeypair", "-keystore", keyStore, "-alias", "turattext",
                 "-keyalg", "RSA", "-keysize", "4096", "-sigalg", "SHA256withRSA",
                 "-validity", "10000", "-storepass", password, "-keypass", password,
                 "-dname", "CN=TuratText Release, OU=Release, O=TuratText, C=RU"
             })
        start.ArgumentList.Add(argument);
    using Process process = Process.Start(start) ?? throw new InvalidOperationException("Could not start keytool");
    process.WaitForExit();
    if (process.ExitCode != 0 || !File.Exists(keyStore))
        throw new InvalidOperationException("keytool failed to create the Android release keystore");
    File.WriteAllText(passwordFile, password, new UTF8Encoding(false));
    File.WriteAllText(
        Path.Combine(fullDirectory, "ANDROID-KEEP-OFFLINE.txt"),
        "Keep the keystore and password together offline. Losing them prevents upgrades of the installed Android app.\n",
        new UTF8Encoding(false));
    Console.WriteLine($"Created Android release keystore in {fullDirectory}");
}

static JsonSerializerOptions JsonOptions(bool indented = false) =>
    new(JsonSerializerDefaults.Web) { WriteIndented = indented };

static void Usage()
{
    Console.Error.WriteLine("Usage:");
    Console.Error.WriteLine("  UpdateAuthority init <offline-directory> [threshold=2]");
    Console.Error.WriteLine("  UpdateAuthority sign <offline-directory> <release-spec.json> <manifest.json>");
    Console.Error.WriteLine("  UpdateAuthority init-android <offline-directory>");
}

sealed record ReleaseSpecification(
    string Channel,
    long Sequence,
    string ReleaseVersion,
    string NotesUrl,
    int ExpiresDays,
    IReadOnlyList<ReleaseArtifactSpecification> Artifacts);

sealed record ReleaseArtifactSpecification(
    string Platform,
    string Architecture,
    string Url,
    string LocalPath);
