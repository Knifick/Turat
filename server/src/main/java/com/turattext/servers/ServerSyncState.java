package com.turattext.servers;

import jakarta.persistence.Column;
import jakarta.persistence.Entity;
import jakarta.persistence.EnumType;
import jakarta.persistence.Enumerated;
import jakarta.persistence.Id;
import jakarta.persistence.JoinColumn;
import jakarta.persistence.MapsId;
import jakarta.persistence.OneToOne;
import jakarta.persistence.Table;

import java.time.Instant;
import java.util.UUID;

@Entity
@Table(name = "server_sync_state")
public class ServerSyncState {
    @Id
    @Column(name = "server_id")
    private UUID serverId;

    @OneToOne
    @MapsId
    @JoinColumn(name = "server_id")
    private ServerNode server;

    @Column(name = "last_event_id", nullable = false)
    private long lastEventId;

    @Column(name = "last_sync_at")
    private Instant lastSyncAt;

    @Enumerated(EnumType.STRING)
    @Column(nullable = false, length = 30)
    private SyncStatus status;

    @Column(name = "error_message", length = 1000)
    private String errorMessage;

    public ServerNode getServer() {
        return server;
    }

    public void setServer(ServerNode server) {
        this.server = server;
    }

    public long getLastEventId() {
        return lastEventId;
    }

    public void setLastEventId(long lastEventId) {
        this.lastEventId = lastEventId;
    }

    public Instant getLastSyncAt() {
        return lastSyncAt;
    }

    public void setLastSyncAt(Instant lastSyncAt) {
        this.lastSyncAt = lastSyncAt;
    }

    public SyncStatus getStatus() {
        return status;
    }

    public void setStatus(SyncStatus status) {
        this.status = status;
    }

    public String getErrorMessage() {
        return errorMessage;
    }

    public void setErrorMessage(String errorMessage) {
        this.errorMessage = errorMessage;
    }
}

