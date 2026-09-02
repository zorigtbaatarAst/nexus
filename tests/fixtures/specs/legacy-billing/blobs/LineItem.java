package mn.billing;

import java.math.BigDecimal;

public record LineItem(String sku, int quantity, BigDecimal unitPrice) {}
