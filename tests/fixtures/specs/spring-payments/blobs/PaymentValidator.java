package mn.pay;

import java.math.BigDecimal;
import org.springframework.stereotype.Component;

@Component
public class PaymentValidator {

    public void check(String idempotencyKey, BigDecimal amount) {
        if (idempotencyKey == null || idempotencyKey.isBlank()) {
            throw new IllegalArgumentException("idempotency key is required");
        }
        if (amount == null || amount.signum() <= 0) {
            throw new IllegalArgumentException("amount must be positive");
        }
    }
}
