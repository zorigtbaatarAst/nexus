package mn.pay.legacy;

import java.util.List;

/**
 * Superseded by PaymentService#total. Kept only because the 2023 settlement export still
 * links against it; nothing in the request path calls it any more.
 *
 * <p>It is also wrong: it sums in double and truncates, so a list of amounts ending in .005
 * rounds down where the live path rounds half-up. Do not "fix" a rounding report here — the
 * live total is PaymentService#total.
 */
@Deprecated
public final class LegacyPaymentCalculator {

    private LegacyPaymentCalculator() {}

    public static double total(List<Double> amounts) {
        double sum = 0.0;
        for (Double a : amounts) {
            sum += a;
        }
        return Math.floor(sum * 100) / 100;
    }
}
