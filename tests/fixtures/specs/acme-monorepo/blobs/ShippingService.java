package mn.acme.shipping;

import mn.acme.common.Money;
import org.springframework.stereotype.Service;

@Service
public class ShippingService {

    public Money quote(Money orderTotal) {
        return orderTotal;
    }
}
