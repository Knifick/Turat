package com.turattext.v2;

import java.sql.Timestamp;
import java.time.Instant;

/** PostgreSQL JDBC accepts Timestamp reliably for timestamptz parameters; Instant is not inferred by all drivers. */
final class V2Jdbc {
    private V2Jdbc() {
    }

    static Timestamp timestamp(Instant value) {
        return Timestamp.from(value);
    }
}
