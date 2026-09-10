package com.turattext.admin;

import com.turattext.admin.dto.BackupResponse;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.jdbc.core.JdbcTemplate;
import org.springframework.scheduling.annotation.Scheduled;
import org.springframework.stereotype.Service;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.attribute.BasicFileAttributes;
import java.time.Instant;
import java.time.ZoneOffset;
import java.time.format.DateTimeFormatter;
import java.util.Comparator;
import java.util.List;
import java.util.zip.ZipEntry;
import java.util.zip.ZipOutputStream;

@Service
public class BackupService {
    private static final DateTimeFormatter FileStamp = DateTimeFormatter
            .ofPattern("yyyyMMdd-HHmmss")
            .withZone(ZoneOffset.UTC);

    private final JdbcTemplate jdbcTemplate;
    private final String datasourceUrl;
    private final boolean enabled;
    private final Path backupDirectory;
    private final Path dataDirectory;

    public BackupService(
            JdbcTemplate jdbcTemplate,
            @Value("${spring.datasource.url:}") String datasourceUrl,
            @Value("${turattext.backup.enabled:true}") boolean enabled,
            @Value("${turattext.backup.directory:./backups}") String backupDirectory
    ) {
        this.jdbcTemplate = jdbcTemplate;
        this.datasourceUrl = datasourceUrl;
        this.enabled = enabled;
        this.backupDirectory = Path.of(backupDirectory);
        this.dataDirectory = Path.of("./data");
    }

    @Scheduled(
            fixedDelayString = "${turattext.backup.interval-ms:21600000}",
            initialDelayString = "${turattext.backup.initial-delay-ms:60000}"
    )
    public void scheduledBackup() throws IOException {
        if (enabled) {
            createBackup();
        }
    }

    public BackupResponse createBackup() throws IOException {
        Files.createDirectories(backupDirectory);
        String stamp = FileStamp.format(Instant.now());
        Path zipPath = backupDirectory.resolve("turattext-backup-" + stamp + ".zip");
        Path tempDirectory = Files.createTempDirectory("turattext-backup-");

        try {
            Path manifest = tempDirectory.resolve("manifest.txt");
            Files.writeString(manifest, "createdAt=" + Instant.now() + System.lineSeparator()
                    + "datasource=" + datasourceUrl + System.lineSeparator());

            if (datasourceUrl.startsWith("jdbc:h2:")) {
                Path scriptPath = tempDirectory.resolve("h2-script.sql");
                String escapedPath = scriptPath.toAbsolutePath().toString().replace("\\", "\\\\").replace("'", "''");
                jdbcTemplate.execute("SCRIPT TO '" + escapedPath + "'");
            } else {
                Files.writeString(tempDirectory.resolve("database-backup-note.txt"),
                        "This datasource is not H2. Use pg_dump for PostgreSQL production backups." + System.lineSeparator());
            }

            try (ZipOutputStream zip = new ZipOutputStream(Files.newOutputStream(zipPath))) {
                zipFile(zip, manifest, "manifest.txt");
                Path script = tempDirectory.resolve("h2-script.sql");
                if (Files.exists(script)) {
                    zipFile(zip, script, "h2-script.sql");
                }
                Path note = tempDirectory.resolve("database-backup-note.txt");
                if (Files.exists(note)) {
                    zipFile(zip, note, "database-backup-note.txt");
                }
                if (Files.exists(dataDirectory)) {
                    zipDirectory(zip, dataDirectory, "data");
                }
            }

            return toResponse(zipPath);
        } finally {
            deleteRecursive(tempDirectory);
        }
    }

    public List<BackupResponse> listBackups() throws IOException {
        if (!Files.exists(backupDirectory)) {
            return List.of();
        }

        try (var stream = Files.list(backupDirectory)) {
            return stream
                    .filter(path -> path.getFileName().toString().endsWith(".zip"))
                    .sorted(Comparator.comparing(this::lastModified).reversed())
                    .map(this::toResponseUnchecked)
                    .toList();
        }
    }

    private void zipDirectory(ZipOutputStream zip, Path directory, String prefix) throws IOException {
        try (var stream = Files.walk(directory)) {
            for (Path path : stream.filter(Files::isRegularFile).toList()) {
                String entryName = prefix + "/" + directory.relativize(path).toString().replace("\\", "/");
                zipFile(zip, path, entryName);
            }
        }
    }

    private static void zipFile(ZipOutputStream zip, Path path, String entryName) throws IOException {
        zip.putNextEntry(new ZipEntry(entryName));
        Files.copy(path, zip);
        zip.closeEntry();
    }

    private BackupResponse toResponseUnchecked(Path path) {
        try {
            return toResponse(path);
        } catch (IOException ex) {
            throw new IllegalStateException(ex);
        }
    }

    private BackupResponse toResponse(Path path) throws IOException {
        BasicFileAttributes attributes = Files.readAttributes(path, BasicFileAttributes.class);
        return new BackupResponse(
                path.getFileName().toString(),
                attributes.size(),
                attributes.creationTime().toInstant()
        );
    }

    private Instant lastModified(Path path) {
        try {
            return Files.getLastModifiedTime(path).toInstant();
        } catch (IOException ex) {
            return Instant.EPOCH;
        }
    }

    private static void deleteRecursive(Path directory) throws IOException {
        if (!Files.exists(directory)) {
            return;
        }

        try (var stream = Files.walk(directory)) {
            for (Path path : stream.sorted(Comparator.reverseOrder()).toList()) {
                Files.deleteIfExists(path);
            }
        }
    }
}
