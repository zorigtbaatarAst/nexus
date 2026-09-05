package mn.shop.api;

import static org.junit.jupiter.api.Assertions.assertEquals;

import java.math.BigDecimal;
import org.junit.jupiter.api.Test;

/**
 * The backend's collateral test: it exists so `mvn test` on this module runs something, and
 * so a change that breaks the DTO is caught rather than merely compiled.
 *
 * <p>It touches only `id`, `reference` and `status`, and constructs the record positionally.
 * The amount field is renamed by Family B's tasks, and a test that named it would fail on a
 * *correct* rename — collateral damage the grader would have no way to tell from real damage.
 */
class OrderDtoTest {

    @Test
    void carries_the_fields_it_was_built_with() {
        OrderDto dto = new OrderDto("o-1", "REF-1", new BigDecimal("12.34"), "PAID");
        assertEquals("o-1", dto.id());
        assertEquals("REF-1", dto.reference());
        assertEquals("PAID", dto.status());
    }
}
