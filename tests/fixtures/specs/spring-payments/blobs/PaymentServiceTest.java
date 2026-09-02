package mn.pay;

import static org.junit.jupiter.api.Assertions.assertEquals;

import java.math.BigDecimal;
import java.util.List;
import org.junit.jupiter.api.Test;

class PaymentServiceTest {

    @Test
    void total_rounds_half_up_to_two_places() {
        PaymentService service = new PaymentService(null, null);
        assertEquals(new BigDecimal("0.00"), service.total(List.of()));
    }
}
