package mn.shop.api;

import jakarta.persistence.Entity;
import jakarta.persistence.Id;
import java.math.BigDecimal;

@Entity
public class Order {

    @Id
    private String id;

    private String reference;

    private BigDecimal grossAmount;

    private String status;

    public String getId() {
        return id;
    }

    public String getReference() {
        return reference;
    }

    public BigDecimal getTotalAmount() {
        return grossAmount;
    }

    public String getStatus() {
        return status;
    }
}
