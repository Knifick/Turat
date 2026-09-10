package com.turattext.v2;

import com.fasterxml.jackson.databind.ObjectMapper;
import jakarta.annotation.PostConstruct;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.stereotype.Service;

import java.io.ByteArrayOutputStream;
import java.io.DataOutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyFactory;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.PrivateKey;
import java.security.Signature;
import java.security.spec.PKCS8EncodedKeySpec;
import java.time.Instant;
import java.time.temporal.ChronoUnit;
import java.util.Base64;
import java.util.List;

@Service
public class NodeIdentityService {
    private static final int DESCRIPTOR_VERSION = 2;
    private final ObjectMapper objectMapper;
    private final Path identityFile;
    private final String nodeName;
    private final String baseUrl;
    private final int registrationPowBits;
    private final int envelopePowBits;
    private final int contactPowBits;
    private final int maxEnvelopeBytes;
    private final long maxMailboxTtlHours;
    private PrivateKey privateKey;
    private String publicKey;
    private String nodeId;

    public NodeIdentityService(
            ObjectMapper objectMapper,
            @Value("${turattext.v2.node-identity-file:./data/node-identity-v2.json}") String identityFile,
            @Value("${turattext.server.name:TuratText Node}") String nodeName,
            @Value("${turattext.server.base-url:http://localhost:8080}") String baseUrl,
            @Value("${turattext.v2.registration-pow-bits:0}") int registrationPowBits,
            @Value("${turattext.v2.envelope-pow-bits:0}") int envelopePowBits,
            @Value("${turattext.v2.contact-pow-bits:0}") int contactPowBits,
            @Value("${turattext.v2.mailbox-max-envelope-bytes:524288}") int maxEnvelopeBytes,
            @Value("${turattext.v2.mailbox-max-ttl-hours:336}") long maxMailboxTtlHours
    ) {
        this.objectMapper = objectMapper;
        this.identityFile = Path.of(identityFile).toAbsolutePath().normalize();
        this.nodeName = sanitize(nodeName);
        this.baseUrl = baseUrl.replaceAll("/+$", "");
        this.registrationPowBits = Math.clamp(registrationPowBits, 0, 28);
        this.envelopePowBits = Math.clamp(envelopePowBits, 0, 24);
        this.contactPowBits = Math.clamp(contactPowBits, 0, 28);
        this.maxEnvelopeBytes = maxEnvelopeBytes;
        this.maxMailboxTtlHours = maxMailboxTtlHours;
    }

    @PostConstruct
    void initialize() throws Exception {
        if (Files.exists(identityFile)) {
            StoredNodeIdentity stored = objectMapper.readValue(identityFile.toFile(), StoredNodeIdentity.class);
            privateKey = KeyFactory.getInstance("Ed25519").generatePrivate(
                    new PKCS8EncodedKeySpec(Base64.getDecoder().decode(stored.privateKey())));
            publicKey = stored.publicKey();
        } else {
            Files.createDirectories(identityFile.getParent());
            KeyPair pair = KeyPairGenerator.getInstance("Ed25519").generateKeyPair();
            privateKey = pair.getPrivate();
            publicKey = Base64.getEncoder().encodeToString(pair.getPublic().getEncoded());
            Path temporary = identityFile.resolveSibling(identityFile.getFileName() + ".new");
            objectMapper.writeValue(temporary.toFile(), new StoredNodeIdentity(
                    Base64.getEncoder().encodeToString(pair.getPrivate().getEncoded()), publicKey));
            try {
                Files.move(temporary, identityFile, java.nio.file.StandardCopyOption.ATOMIC_MOVE);
            } catch (java.nio.file.AtomicMoveNotSupportedException ignored) {
                Files.move(temporary, identityFile, java.nio.file.StandardCopyOption.REPLACE_EXISTING);
            }
        }
        nodeId = "ttn1-" + V2Encoding.sha256Hex(Base64.getDecoder().decode(publicKey));
    }

    public NodeDescriptor descriptor() {
        long expiresAt = Instant.now().plus(7, ChronoUnit.DAYS).toEpochMilli();
        List<String> transports = List.of("https", "http2", "relay-tls");
        byte[] canonical = canonicalDescriptor(
                DESCRIPTOR_VERSION, nodeId, nodeName, baseUrl, publicKey, expiresAt, transports,
                registrationPowBits, envelopePowBits, contactPowBits, maxEnvelopeBytes, maxMailboxTtlHours);
        return new NodeDescriptor(
                DESCRIPTOR_VERSION,
                nodeId,
                nodeName,
                baseUrl,
                publicKey,
                "Ed25519",
                expiresAt,
                transports,
                registrationPowBits,
                envelopePowBits,
                contactPowBits,
                maxEnvelopeBytes,
                maxMailboxTtlHours,
                sign(canonical));
    }

    public String sign(byte[] value) {
        try {
            Signature signer = Signature.getInstance("Ed25519");
            signer.initSign(privateKey);
            signer.update(value);
            return Base64.getEncoder().encodeToString(signer.sign());
        } catch (Exception exception) {
            throw new IllegalStateException("Could not sign with node identity", exception);
        }
    }

    public static byte[] canonicalDescriptor(
            int version,
            String nodeId,
            String name,
            String baseUrl,
            String publicKey,
            long expiresAt,
            List<String> transports,
            int registrationPowBits,
            int envelopePowBits,
            int contactPowBits,
            int maxEnvelopeBytes,
            long maxMailboxTtlHours
    ) {
        try {
            var bytes = new ByteArrayOutputStream();
            var output = new DataOutputStream(bytes);
            writeString(output, "TuratText.NodeDescriptor");
            output.writeInt(version);
            writeString(output, nodeId);
            writeString(output, name);
            writeString(output, baseUrl);
            writeString(output, publicKey);
            output.writeLong(expiresAt);
            output.writeInt(transports.size());
            for (String transport : transports) writeString(output, transport);
            output.writeInt(registrationPowBits);
            output.writeInt(envelopePowBits);
            output.writeInt(contactPowBits);
            output.writeInt(maxEnvelopeBytes);
            output.writeLong(maxMailboxTtlHours);
            output.flush();
            return bytes.toByteArray();
        } catch (Exception exception) {
            throw new IllegalStateException("Could not encode node descriptor", exception);
        }
    }

    private static void writeString(DataOutputStream output, String value) throws Exception {
        byte[] bytes = value.getBytes(StandardCharsets.UTF_8);
        output.writeInt(bytes.length);
        output.write(bytes);
    }

    private static String sanitize(String value) {
        return value.replace('\n', ' ').replace('\r', ' ').trim();
    }

    private record StoredNodeIdentity(String privateKey, String publicKey) {
    }

    public record NodeDescriptor(
            int version,
            String nodeId,
            String name,
            String baseUrl,
            String publicKey,
            String algorithm,
            long expiresAtUnixMilliseconds,
            List<String> transports,
            int registrationPowBits,
            int envelopePowBits,
            int contactPowBits,
            int maxEnvelopeBytes,
            long maxMailboxTtlHours,
            String signature
    ) {
    }
}
