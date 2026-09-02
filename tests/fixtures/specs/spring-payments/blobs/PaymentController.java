package mn.pay;

import java.math.BigDecimal;
import java.util.List;
import org.springframework.graphql.data.method.annotation.Argument;
import org.springframework.graphql.data.method.annotation.MutationMapping;
import org.springframework.graphql.data.method.annotation.QueryMapping;
import org.springframework.stereotype.Controller;

@Controller
public class PaymentController {

    private final PaymentService service;
    private final PaymentRepository repository;

    public PaymentController(PaymentService service, PaymentRepository repository) {
        this.service = service;
        this.repository = repository;
    }

    @QueryMapping
    public List<PaymentDto> payments() {
        return repository.findAll().stream().map(PaymentDto::of).toList();
    }

    @MutationMapping
    public PaymentDto createPayment(
            @Argument String idempotencyKey, @Argument BigDecimal amount) {
        return PaymentDto.of(service.create(idempotencyKey, amount));
    }
}
