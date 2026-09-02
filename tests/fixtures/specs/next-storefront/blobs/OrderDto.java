package mn.shop.api;

import java.math.BigDecimal;

public record OrderDto(String id, String reference, BigDecimal totalAmount, String status) {

    public static OrderDto of(Order o) {
        return new OrderDto(o.getId(), o.getReference(), o.getTotalAmount(), o.getStatus());
    }
}
