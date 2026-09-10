namespace TuratText.Client.Updates;

// This public 2-of-3 root is compiled into the client. The corresponding private
// keys live only under release-secrets/update-authority and must remain offline.
public static class ReleaseTrustRoot
{
    public static UpdateTrustRoot Current { get; } = new(
        1,
        2,
        [
            new UpdateTrustKey(
                "upd1-ff8ddb2dcb960f7bb2afe2ff",
                "Ed25519",
                "MCowBQYDK2VwAyEAqOiaB0jNWnSQZ+r33X8uopWvvUXNgF+wbUtBSpEpjNs="),
            new UpdateTrustKey(
                "upd1-a9c1579ac61362ccfd24edd3",
                "Ed25519",
                "MCowBQYDK2VwAyEAxczZ+aj2bcJM7d60dc7Wf4RldCnVTmw24BTj18wu4j8="),
            new UpdateTrustKey(
                "upd1-1a94a72e3a587f5bc66331c0",
                "Ed25519",
                "MCowBQYDK2VwAyEAdYs6uLG7fcDjJ7Cpwa21CjOw97dolp6k9fyPQfnSIo4=")
        ]);
}
