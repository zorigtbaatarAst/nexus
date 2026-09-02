package mn.billing;

import static org.junit.jupiter.api.Assertions.assertEquals;

import java.math.BigDecimal;
import java.util.List;
import org.junit.jupiter.api.Test;

class InvoiceServiceTest {

    private final InvoiceService service = new InvoiceService(new TaxTable());

    @Test
    void subtotal_multiplies_quantity_by_unit_price() {
        List<LineItem> items = List.of(new LineItem("A", 3, new BigDecimal("1.50")));
        assertEquals(new BigDecimal("4.50"), service.subtotal(items));
    }
}
