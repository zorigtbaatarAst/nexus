package mn.acme.orders;

import jakarta.persistence.Entity;
import mn.acme.common.BaseEntity;
import mn.acme.common.Money;

@Entity
public class OrderEntity extends BaseEntity {

    private String customerId;

    private Money total;

    public String getCustomerId() {
        return customerId;
    }

    public Money getTotal() {
        return total;
    }
}
