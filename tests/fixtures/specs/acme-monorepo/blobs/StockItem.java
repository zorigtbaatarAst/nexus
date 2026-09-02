package mn.acme.inventory;

import jakarta.persistence.Entity;
import mn.acme.common.BaseEntity;
import mn.acme.common.Money;

@Entity
public class StockItem extends BaseEntity {

    private String sku;

    private int onHand;

    private Money unitPrice;

    public String getSku() {
        return sku;
    }

    public int getOnHand() {
        return onHand;
    }

    public Money getUnitPrice() {
        return unitPrice;
    }
}
