package mn.acme.common;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

import java.math.BigDecimal;
import org.junit.jupiter.api.Test;

/**
 * The shared library's collateral test: it exists so `gradle test` runs something, and so a
 * change to the type every service embeds is caught rather than merely compiled.
 *
 * <p>Both assertions are scale-independent — `compareTo`, not `equals`. A2 changes Money's
 * scale from 2 to 4, and a test that pinned the scale would fail on a *correct* fix, which
 * the grader would score as collateral damage.
 */
class MoneyTest {

    @Test
    void adds_amounts_in_the_same_currency() {
        Money a = new Money(new BigDecimal("1.50"), "MNT");
        Money b = new Money(new BigDecimal("2.25"), "MNT");
        assertEquals(0, a.plus(b).amount().compareTo(new BigDecimal("3.75")));
    }

    @Test
    void refuses_to_add_across_currencies() {
        Money mnt = new Money(BigDecimal.ONE, "MNT");
        Money usd = new Money(BigDecimal.ONE, "USD");
        assertThrows(IllegalArgumentException.class, () -> mnt.plus(usd));
    }
}
