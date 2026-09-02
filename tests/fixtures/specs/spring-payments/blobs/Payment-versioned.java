package mn.payments;

import jakarta.persistence.Column;
import jakarta.persistence.Entity;
import jakarta.persistence.Id;
import jakarta.persistence.Version;
import java.math.BigDecimal;

@Entity
public class Payment {

    @Id
    private String id;

    // Optimistic locking: two concurrent settlements of the same payment can no longer
    // both win. Added with V2__payment_unique_index.sql, which closes the other half.
    @Version
    private Long version;

    @Column(nullable = false)
    private String idempotencyKey;

    @Column(nullable = false)
    private BigDecimal amount;

    @Column(nullable = false)
    private String status;

    public String getId() {
        return id;
    }

    public Long getVersion() {
        return version;
    }

    public String getIdempotencyKey() {
        return idempotencyKey;
    }

    public BigDecimal getAmount() {
        return amount;
    }

    public String getStatus() {
        return status;
    }

    public void setStatus(String status) {
        this.status = status;
    }
}
