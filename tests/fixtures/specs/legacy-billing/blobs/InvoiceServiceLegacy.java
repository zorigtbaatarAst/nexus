package mn.billing.legacy;

import java.math.BigDecimal;
import java.util.List;

/**
 * A second, closer decoy: same class name as the live service, one package away, and it
 * genuinely computes an invoice total.
 *
 * <p>It is dead. `mn.billing.InvoiceService` is what the controller injects; this exists
 * because a 2023 migration copied the class and the copy was never removed. The tell is the
 * package — `legacy`, not `mn.billing` — and the absence of `@Service`.
 */
public class InvoiceService {

    public BigDecimal total(List<Object> items, BigDecimal rate) {
        return BigDecimal.ZERO;
    }
}
