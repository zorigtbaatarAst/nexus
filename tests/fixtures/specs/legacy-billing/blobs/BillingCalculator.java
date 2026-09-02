package mn.billing.v1;

import java.math.BigDecimal;
import java.math.RoundingMode;
import java.util.List;

/**
 * The original billing calculator, from before the tax rework. Retained because the 2022
 * archive reports still deserialize against it.
 *
 * <p>Nothing calls it. There is no bean, no controller, no scheduled job — the only reference
 * anywhere is this comment. It is kept in the tree because deleting it broke an offline report
 * once and nobody has been brave enough since.
 *
 * <p>It also computes tax on the pre-discount subtotal, which is the behaviour the tax rework
 * was done to change. A "fix" applied here changes nothing anyone can observe.
 */
@Deprecated(since = "2.0", forRemoval = false)
public class BillingCalculator {

    public BigDecimal subtotal(List<Object[]> rows) {
        BigDecimal sum = BigDecimal.ZERO;
        for (Object[] r : rows) {
            sum = sum.add((BigDecimal) r[2]);
        }
        return sum.setScale(2, RoundingMode.HALF_UP);
    }

    public BigDecimal total(List<Object[]> rows, BigDecimal taxRate) {
        BigDecimal sub = subtotal(rows);
        return sub.add(sub.multiply(taxRate)).setScale(2, RoundingMode.HALF_UP);
    }
}
