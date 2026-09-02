package mn.acme.common;

import java.math.BigDecimal;
import java.math.RoundingMode;

/**
 * The type every service in this repository extends or embeds. An edit here reaches all of
 * them, which is exactly the reach that a scan of one module cannot see.
 */
public final class Money {

    private final BigDecimal amount;
    private final String currency;

    public Money(BigDecimal amount, String currency) {
        this.amount = amount.setScale(2, RoundingMode.HALF_UP);
        this.currency = currency;
    }

    public BigDecimal amount() {
        return amount;
    }

    public String currency() {
        return currency;
    }

    public Money plus(Money other) {
        if (!currency.equals(other.currency)) {
            throw new IllegalArgumentException("cannot add " + currency + " to " + other.currency);
        }
        return new Money(amount.add(other.amount), currency);
    }
}
