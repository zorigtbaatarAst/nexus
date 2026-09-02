package mn.billing;

import java.math.BigDecimal;
import java.util.Map;
import org.springframework.stereotype.Component;

@Component
public class TaxTable {

    // The Mongolian VAT rate moved to 12% in the 2025 budget.
    private static final Map<String, BigDecimal> RATES =
            Map.of("MN", new BigDecimal("0.12"), "US-CA", new BigDecimal("0.0725"));

    public BigDecimal rateFor(String region) {
        return RATES.getOrDefault(region, BigDecimal.ZERO);
    }
}
