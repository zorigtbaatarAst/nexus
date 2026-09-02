package mn.acme.inventory;

import mn.acme.common.Money;
import org.springframework.stereotype.Service;

@Service
public class InventoryService {

    public boolean canFulfil(StockItem item, int quantity) {
        return item.getOnHand() >= quantity;
    }

    public Money valueOnHand(StockItem item) {
        return item.getUnitPrice();
    }
}
