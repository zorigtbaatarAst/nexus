package mn.acme.common;

import jakarta.persistence.Id;
import jakarta.persistence.MappedSuperclass;
import java.time.Instant;

/** Extended by every entity in every service. */
@MappedSuperclass
public abstract class BaseEntity {

    @Id
    protected String id;

    protected Instant createdAt;

    public String getId() {
        return id;
    }

    public Instant getCreatedAt() {
        return createdAt;
    }
}
