package mn.payments;

import java.math.BigDecimal;
import java.math.RoundingMode;
import java.util.List;
import org.springframework.stereotype.Service;
import org.springframework.transaction.annotation.Transactional;

@Service
public class PaymentService {

    private final PaymentRepository repository;
    private final PaymentValidator validator;

    public PaymentService(PaymentRepository repository, PaymentValidator validator) {
        this.repository = repository;
        this.validator = validator;
    }

    /**
     * The check is back inside the transaction, and V2__payment_unique_index.sql makes the
     * database the final arbiter: the application check is now an optimisation, not the
     * guarantee. Both halves are needed — the index alone turns a duplicate into an
     * exception, and the transaction alone still races on a cluster.
     */
    @Transactional
    public Payment create(String idempotencyKey, BigDecimal amount) {
        validator.check(idempotencyKey, amount);
        if (repository.existsByIdempotencyKey(idempotencyKey)) {
            return repository.findByIdempotencyKey(idempotencyKey).orElseThrow();
        }
        Payment p = new Payment();
        p.setStatus("PENDING");
        return repository.save(p);
    }

    /** The live total. Rounds half-up to two places, which is what the ledger expects. */
    public BigDecimal total(List<Payment> payments) {
        BigDecimal sum = BigDecimal.ZERO;
        for (Payment p : payments) {
            sum = sum.add(p.getAmount());
        }
        return sum.setScale(2, RoundingMode.HALF_UP);
    }
}
