package mn.billing;

import java.math.BigDecimal;
import java.util.Map;
import org.springframework.stereotype.Component;

@Component
public class TaxTable {

    private static final Map<String, BigDecimal> RATES =
            Map.of("MN", new BigDecimal("0.10"), "US-CA", new BigDecimal("0.0725"));

    public BigDecimal rateFor(String region) {
        return RATES.getOrDefault(region, BigDecimal.ZERO);
    }
}
