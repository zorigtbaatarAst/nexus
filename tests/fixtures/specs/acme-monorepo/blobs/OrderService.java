package mn.acme.orders;

import java.util.List;
import mn.acme.common.Money;
import org.springframework.stereotype.Service;

@Service
public class OrderService {

    public Money total(List<OrderEntity> orders) {
        Money sum = null;
        for (OrderEntity o : orders) {
            sum = sum == null ? o.getTotal() : sum.plus(o.getTotal());
        }
        return sum;
    }
}
