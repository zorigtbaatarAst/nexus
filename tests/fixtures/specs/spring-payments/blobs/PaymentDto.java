package mn.pay;

import java.math.BigDecimal;

public record PaymentDto(String id, BigDecimal amount, String status) {

    public static PaymentDto of(Payment p) {
        return new PaymentDto(p.getId(), p.getAmount(), p.getStatus());
    }
}
