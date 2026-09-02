package mn.billing;

import java.math.BigDecimal;
import java.math.RoundingMode;
import java.util.List;
import org.springframework.stereotype.Service;

/**
 * The live billing path. Everything the HTTP layer reaches goes through here.
 */
@Service
public class InvoiceService {

    private final TaxTable taxTable;

    public InvoiceService(TaxTable taxTable) {
        this.taxTable = taxTable;
    }

    public BigDecimal subtotal(List<LineItem> items) {
        BigDecimal sum = BigDecimal.ZERO;
        for (LineItem i : items) {
            sum = sum.add(i.unitPrice().multiply(BigDecimal.valueOf(i.quantity())));
        }
        return sum.setScale(2, RoundingMode.HALF_UP);
    }

    public BigDecimal tax(List<LineItem> items, String region) {
        return subtotal(items)
                .multiply(taxTable.rateFor(region))
                .setScale(2, RoundingMode.HALF_UP);
    }

    public BigDecimal total(List<LineItem> items, String region) {
        return subtotal(items).add(tax(items, region));
    }
}
